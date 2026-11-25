use std::{
	io,
	os::unix::ffi::OsStrExt,
	path::{Path, absolute},
};

use dmgez_inspector::volume_queries;
use dmgez_operations::extract::{
	ExtractionLimits, HostName, Selection, execute, host::StableDestination, prepare,
};
use dmgez_vfs::volume::ForkKind;

use crate::{CliResult, volume_input::with_hfs_view_verified};

use super::{Arguments, Fork, MAX_BYTES_OPTION};

mod report;

use report::Report;

const PATH_SEPARATOR: u8 = b'/';
const CURRENT_COMPONENT: &[u8] = b".";
const PARENT_COMPONENT: &[u8] = b"..";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let selection = match arguments.projection.fork {
		Some(Fork::Data) => Selection::Fork(ForkKind::Data),
		Some(Fork::Resource) => Selection::Fork(ForkKind::Resource),
		None => Selection::DataTree,
	};
	let output = absolute(&arguments.output)?;
	let mut report = Report::new(&arguments.path, &output);
	let result = prepare_and_run(arguments, &output, selection, &mut report);
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
	let input = &arguments.input;
	with_hfs_view_verified(
		&input.image,
		input.raw,
		input.partition,
		|view, verify_source| {
			let node = volume_queries::resolve_node(view, &arguments.path)?;
			let plan = prepare(view, &node, selection, limits, || false)?;
			report.plan = Some(*plan.report());
			if arguments.dry_run {
				verify_source()?;
				directory.require_absent(&name)?;
				return Ok(());
			}
			match execute(view, &plan, &directory, &name, verify_source, || false) {
				Ok(progress) => {
					report.progress = Some(progress);
					Ok(())
				}
				Err(error) => {
					report.progress = Some(error.progress());
					Err(error.into())
				}
			}
		},
	)
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
