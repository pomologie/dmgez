use std::{
	borrow::Cow,
	hash::Hash,
	io::{self, Write},
	os::unix::ffi::OsStrExt,
	path::{Path, absolute},
};

use dmgez_config::VirtualPath;
use dmgez_inspector::SCHEMA_VERSION;
use dmgez_operations::{
	extract::{NameLimits, NameProfile, host::StableDestination},
	install::{
		CopyLimits, CopySummary, execute_copy, execute_copy_with_check, prepare_application,
		prepare_copy,
	},
};
use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use serde::Serialize;

#[cfg(feature = "hfsplus-install")]
use crate::volume_input::with_hfs_view_verified;
use crate::{
	CliResult,
	report_output::{EscapedBytes, EscapedText, write_report},
};
#[cfg(feature = "apfs-install")]
use crate::{
	volume_commands::{VolumeIdentity, with_selector_guidance},
	volume_input::with_apfs_view_verified,
};

use super::{Arguments, COMMAND, MAX_BYTES_OPTION};

mod policy;

const STAGING_DEPTH: usize = 2;

#[cfg(feature = "hfsplus-install")]
pub(super) fn hfsplus(arguments: &Arguments) -> CliResult {
	run(arguments, |report| {
		let input = &arguments.input;
		with_hfs_view_verified(
			&input.image,
			input.raw,
			input.partition,
			|view, verify_source| copy(view, verify_source, arguments, report),
		)
	})
}

#[cfg(feature = "apfs-install")]
pub(super) fn apfs(arguments: &Arguments) -> CliResult {
	run(arguments, |report| {
		let input = &arguments.input;
		with_apfs_view_verified(
			&input.image,
			input.raw,
			input.partition,
			arguments.filesystem.apfs_volume()?,
			arguments.filesystem.apfs_checkpoint(),
			|view, info, verify_source| {
				report.apfs_volume = Some(VolumeIdentity::from(info));
				copy(view, verify_source, arguments, report)
			},
		)
		.map_err(with_selector_guidance)
	})
}

fn run(arguments: &Arguments, perform: impl FnOnce(&mut Report<'_>) -> CliResult) -> CliResult {
	let directory = absolute(&arguments.destination)?;
	let mut report = Report::new(arguments, &directory);
	let result = perform(&mut report);
	report.status = match (&result, arguments.dry_run, report.published) {
		(Ok(()), true, _) => Status::Planned,
		(Ok(()), false, _) => Status::Complete,
		(Err(_), _, true) => Status::PublishedWithError,
		(Err(_), _, false) => Status::Failed,
	};
	if let Err(error) = &result {
		report.error = Some(error.to_string());
	}
	let rendered = write_report(arguments.json, &report, |output| report.write(output));
	result.and(rendered)
}

fn copy<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	verify_source: &dyn Fn() -> io::Result<()>,
	arguments: &Arguments,
	report: &mut Report<'_>,
) -> CliResult
where
	B::Id: Eq + Hash,
{
	let source = arguments.app.parse::<VirtualPath>()?;
	let mut names = NameLimits::default();
	names.max_depth -= STAGING_DEPTH;
	let application =
		prepare_application(view, &source, NameProfile::PortableAscii, names, || false)?;
	report.bundle_name = Some(application.destination_name().as_str().to_owned());
	let mut limits = CopyLimits::default();
	if let Some(bytes) = arguments.max_bytes {
		if bytes > limits.max_payload_bytes {
			return Err(
				format!("--{MAX_BYTES_OPTION} exceeds {}", limits.max_payload_bytes).into(),
			);
		}
		limits.max_payload_bytes = bytes;
	}
	let plan = prepare_copy(view, application, limits, || false)?;
	report.plan = Some(Plan::from(plan.summary()));
	plan.require_host_metadata()?;
	let directory = StableDestination::open(report.destination)?;
	plan.application().require_fresh_destination(&directory)?;
	if arguments.dry_run {
		verify_source()?;
		return Ok(());
	}
	let destination = directory.directory();
	let result = if report.checks.requested() {
		execute_copy_with_check(
			view,
			&plan,
			&destination,
			verify_source,
			|| false,
			|path| report.checks.verify(path),
		)
	} else {
		execute_copy(view, &plan, &destination, verify_source, || false)
	};
	match result {
		Ok(_) => {
			report.published = true;
			Ok(())
		}
		Err(error) => {
			report.published = error.published();
			report.residual_stage_name = error.residual_name().map(|name| name.as_str().to_owned());
			Err(error.into())
		}
	}
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
	Planned,
	Complete,
	Failed,
	PublishedWithError,
}

impl Status {
	const fn as_str(self) -> &'static str {
		match self {
			Self::Planned => "planned",
			Self::Complete => "complete",
			Self::Failed => "failed",
			Self::PublishedWithError => "published_with_error",
		}
	}
}

#[derive(Serialize)]
struct Plan {
	payload_bytes: u64,
	attributes: usize,
}

impl From<CopySummary> for Plan {
	fn from(summary: CopySummary) -> Self {
		Self {
			payload_bytes: summary.payload_bytes,
			attributes: summary.attributes,
		}
	}
}

#[derive(Serialize)]
struct Report<'input> {
	schema_version: u32,
	operation: &'static str,
	status: Status,
	source_path: &'input str,
	#[serde(skip)]
	destination: &'input Path,
	destination_directory: Cow<'input, str>,
	#[serde(skip_serializing_if = "Option::is_none")]
	destination_bytes: Option<&'input [u8]>,
	bundle_name: Option<String>,
	#[cfg(feature = "apfs-install")]
	#[serde(skip_serializing_if = "Option::is_none")]
	apfs_volume: Option<VolumeIdentity>,
	plan: Option<Plan>,
	#[serde(flatten)]
	checks: policy::Checks,
	published: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	residual_stage_name: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	error: Option<String>,
}

impl<'input> Report<'input> {
	fn new(arguments: &'input Arguments, destination: &'input Path) -> Self {
		Self {
			schema_version: SCHEMA_VERSION,
			operation: COMMAND,
			status: Status::Failed,
			source_path: &arguments.app,
			destination,
			destination_directory: destination.to_string_lossy(),
			destination_bytes: destination
				.to_str()
				.is_none()
				.then_some(destination.as_os_str().as_bytes()),
			bundle_name: None,
			#[cfg(feature = "apfs-install")]
			apfs_volume: None,
			plan: None,
			checks: policy::Checks::new(arguments),
			published: false,
			residual_stage_name: None,
			error: None,
		}
	}

	fn write(&self, output: &mut impl Write) -> io::Result<()> {
		writeln!(output, "status = {}", self.status.as_str())?;
		writeln!(output, "source_path = {}", EscapedText(self.source_path))?;
		writeln!(
			output,
			"destination_directory = {}",
			EscapedBytes(self.destination.as_os_str().as_bytes())
		)?;
		if let Some(name) = &self.bundle_name {
			writeln!(output, "bundle_name = {}", EscapedText(name))?;
		}
		#[cfg(feature = "apfs-install")]
		if let Some(volume) = self.apfs_volume {
			volume.write(output)?;
		}
		if let Some(plan) = &self.plan {
			writeln!(output, "planned_payload_bytes = {}", plan.payload_bytes)?;
			writeln!(output, "planned_attributes = {}", plan.attributes)?;
		}
		writeln!(output, "published = {}", self.published)?;
		self.checks.write(output)?;
		if let Some(name) = &self.residual_stage_name {
			writeln!(output, "residual_stage_name = {}", EscapedText(name))?;
		}
		Ok(())
	}
}
