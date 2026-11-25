use std::io;

use dmgez_runtime::extraction::{DestinationError, Evidence, OutputPath, Selection};
use dmgez_vfs::volume::ForkKind;

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
	let output = OutputPath::new(&arguments.output)?;
	let mut report = Report::new(&arguments.path, output.path());
	let result = project(arguments, &output, selection, backend, &mut report);
	match &result {
		Ok(()) => report.finish(arguments.dry_run),
		Err(error) => report.fail(error.as_ref()),
	}
	let rendered = report.write(arguments.json);
	result.and(rendered)
}

fn project(
	arguments: &Arguments,
	output: &OutputPath<'_>,
	selection: Selection,
	backend: Backend,
	report: &mut Report<'_>,
) -> CliResult {
	let destination = output
		.admit(selection, arguments.max_bytes)
		.map_err(output_error)?;
	let input = &arguments.input;
	let mut evidence = Evidence::default();
	let result = match backend {
		#[cfg(feature = "hfsplus-extract")]
		Backend::Hfsplus => with_hfs_view_verified(
			&input.image,
			input.raw,
			input.partition,
			|view, verify_source| {
				destination.project(
					view,
					&arguments.path,
					arguments.dry_run,
					verify_source,
					|| false,
					&mut evidence,
				)?;
				Ok(())
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
				destination.project(
					view,
					&arguments.path,
					arguments.dry_run,
					verify_source,
					|| false,
					&mut evidence,
				)?;
				Ok(())
			},
		)
		.map_err(with_selector_guidance),
	};
	report.plan = evidence.plan;
	report.progress = evidence.progress;
	result
}

fn output_error(error: DestinationError) -> Box<dyn std::error::Error> {
	match error {
		DestinationError::PayloadLimit { maximum, .. } => io::Error::new(
			io::ErrorKind::InvalidInput,
			format!("--{MAX_BYTES_OPTION} cannot exceed {maximum} bytes"),
		)
		.into(),
		_ => error.into(),
	}
}
