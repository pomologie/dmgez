use std::process::ExitCode;

use dmgez_runtime::installer_test::{self, Filesystem, SecurityRequirements};

use crate::{CliResult, volume_commands::Filesystem as FilesystemArgument};

use super::Arguments;

mod report;

pub(super) fn run(arguments: &Arguments) -> CliResult<ExitCode> {
	let filesystem = match arguments.selection.filesystem() {
		FilesystemArgument::Hfsplus => Filesystem::Hfsplus,
		FilesystemArgument::Apfs => Filesystem::Apfs {
			#[cfg(feature = "apfs-read")]
			selector: arguments.selection.apfs_volume(),
			#[cfg(feature = "apfs-read")]
			checkpoint: arguments.selection.apfs_checkpoint(),
		},
	};
	let report = installer_test::run(
		&arguments.image,
		filesystem,
		arguments.partition,
		SecurityRequirements {
			signature: arguments.require_signature,
			ticket: arguments.require_ticket,
			native_assessment: arguments.require_native_assessment,
		},
	)?;
	report::write(arguments, &report)
}
