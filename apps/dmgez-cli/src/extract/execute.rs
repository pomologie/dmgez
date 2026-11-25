use std::{
	hash::Hash,
	io,
	os::unix::ffi::OsStrExt,
	path::{Path, absolute},
};

use dmgez_inspector::volume_queries;
use dmgez_operations::extract::{
	ExtractionLimits, HostName, Selection, execute,
	host::{DestinationDirectory, StableDestination},
	prepare,
};
use dmgez_vfs::volume::{ForkKind, VolumeBackend, VolumeView};

use crate::CliResult;
#[cfg(feature = "hfsplus-extract")]
use crate::volume_input::with_hfs_view_verified;
#[cfg(feature = "apfs-extract")]
use crate::{
	volume_commands::{VolumeIdentity, with_selector_guidance},
	volume_input::with_apfs_view_verified,
};

use super::{Arguments, Fork, MAX_BYTES_OPTION};

mod report;

use report::Report;

const PATH_SEPARATOR: u8 = b'/';
const CURRENT_COMPONENT: &[u8] = b".";
const PARENT_COMPONENT: &[u8] = b"..";

enum Backend {
	#[cfg(feature = "hfsplus-extract")]
	Hfsplus,
	#[cfg(feature = "apfs-extract")]
	Apfs {
		volume: Option<dmgez_apfs_read::VolumeSelector>,
		xid: Option<dmgez_apfs_read::TransactionId>,
	},
}

#[cfg(feature = "hfsplus-extract")]
pub(super) fn hfsplus(arguments: &Arguments) -> CliResult {
	run(arguments, Backend::Hfsplus)
}

#[cfg(feature = "apfs-extract")]
pub(super) fn apfs(arguments: &Arguments) -> CliResult {
	run(
		arguments,
		Backend::Apfs {
			volume: arguments.filesystem.apfs_volume()?,
			xid: arguments.filesystem.apfs_checkpoint(),
		},
	)
}

fn run(arguments: &Arguments, backend: Backend) -> CliResult {
	let selection = match arguments.projection.fork {
		Some(Fork::Data) => Selection::Fork(ForkKind::Data),
		Some(Fork::Resource) => Selection::Fork(ForkKind::Resource),
		None => Selection::DataTree,
	};
	let output = absolute(&arguments.output)?;
	let mut report = Report::new(&arguments.path, &output);
	let result = prepare_and_run(arguments, &output, selection, backend, &mut report);
	match &result {
		Ok(()) => report.finish(arguments.dry_run),
		Err(error) => report.fail(error.as_ref()),
	}
	let rendered = report.write(arguments.json);
	result.and(rendered)
}

fn prepare_and_run(
	arguments: &Arguments,
	output: &Path,
	selection: Selection,
	backend: Backend,
	report: &mut Report<'_>,
) -> CliResult {
	check_destination(&arguments.output, selection)?;
	let parent = output.parent().ok_or_else(invalid_destination)?;
	let name = output
		.file_name()
		.and_then(|name| name.to_str())
		.ok_or_else(invalid_destination)?;
	let limits = extraction_limits(arguments.max_bytes)?;
	let name = HostName::new(name, limits.profile, limits.names.max_component_bytes)?;
	let root = StableDestination::open(parent)?;
	let directory = root.directory();
	directory.require_absent(&name)?;
	let request = Request {
		source_path: &arguments.path,
		selection,
		limits,
		dry_run: arguments.dry_run,
	};
	let input = &arguments.input;
	match backend {
		#[cfg(feature = "hfsplus-extract")]
		Backend::Hfsplus => with_hfs_view_verified(
			&input.image,
			input.raw,
			input.partition,
			|view, verify_source| {
				execute_plan(view, verify_source, &request, &directory, &name, report)
			},
		),
		#[cfg(feature = "apfs-extract")]
		Backend::Apfs { volume, xid } => with_apfs_view_verified(
			&input.image,
			input.raw,
			input.partition,
			volume,
			xid,
			|view, info, verify_source| {
				report.apfs_volume = Some(VolumeIdentity::from(info));
				execute_plan(view, verify_source, &request, &directory, &name, report)
			},
		)
		.map_err(with_selector_guidance),
	}
}

struct Request<'a> {
	source_path: &'a str,
	selection: Selection,
	limits: ExtractionLimits,
	dry_run: bool,
}

fn execute_plan<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	verify_source: &dyn Fn() -> io::Result<()>,
	request: &Request<'_>,
	directory: &DestinationDirectory<'_>,
	name: &HostName,
	report: &mut Report<'_>,
) -> CliResult
where
	B::Id: Eq + Hash,
{
	let node = volume_queries::resolve_node(view, request.source_path)?;
	let plan = prepare(view, &node, request.selection, request.limits, || false)?;
	report.plan = Some(*plan.report());
	if request.dry_run {
		verify_source()?;
		directory.require_absent(name)?;
		return Ok(());
	}
	match execute(view, &plan, directory, name, verify_source, || false) {
		Ok(progress) => {
			report.progress = Some(progress);
			Ok(())
		}
		Err(error) => {
			report.progress = Some(error.progress());
			Err(error.into())
		}
	}
}

fn check_destination(path: &Path, selection: Selection) -> io::Result<()> {
	let mut components = path
		.as_os_str()
		.as_bytes()
		.rsplit(|&byte| byte == PATH_SEPARATOR);
	let last = match selection {
		Selection::DataTree => components.find(|component| !component.is_empty()),
		_ => components.next().filter(|component| !component.is_empty()),
	};
	match last {
		Some(component) if !matches!(component, CURRENT_COMPONENT | PARENT_COMPONENT) => Ok(()),
		_ => Err(invalid_destination()),
	}
}

fn invalid_destination() -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidInput,
		"extraction output must name an absent entry under an existing parent",
	)
}

fn extraction_limits(max_bytes: Option<u64>) -> io::Result<ExtractionLimits> {
	let mut limits = ExtractionLimits::default();
	if let Some(max_bytes) = max_bytes {
		if max_bytes > limits.max_total_bytes {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!(
					"--{MAX_BYTES_OPTION} cannot exceed {} bytes",
					limits.max_total_bytes
				),
			));
		}
		limits.max_total_bytes = max_bytes;
	}
	Ok(limits)
}
