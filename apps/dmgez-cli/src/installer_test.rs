use std::{num::NonZeroU32, path::PathBuf, process::ExitCode};

use clap::{Args, ValueEnum};

use crate::{
	CliResult,
	volume_commands::{PARTITION_SLOT_VALUE_NAME, Selection},
};

pub(super) const FEATURE: &str = "installer-test";
pub(super) const COMPILED: bool = cfg!(all(
	feature = "installer-test",
	any(feature = "hfsplus-read", feature = "apfs-read"),
	any(target_os = "linux", target_os = "macos")
));

#[derive(Args)]
pub(super) struct Arguments {
	/// Finished UDIF disk image to test without mounting or running applications.
	image: PathBuf,
	#[arg(long, value_enum)]
	profile: Profile,
	#[command(flatten)]
	selection: Selection,
	/// Select a one-based partition slot. Otherwise one data window is required.
	#[arg(long, value_name = PARTITION_SLOT_VALUE_NAME)]
	partition: Option<NonZeroU32>,
	/// Require a valid static signature on the DMG.
	#[arg(long)]
	require_signature: bool,
	/// Require native validation of a stapled notarization ticket.
	#[arg(long)]
	require_ticket: bool,
	/// Require an affirmative macOS native policy verdict.
	#[arg(long)]
	require_native_assessment: bool,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Profile {
	Installer,
}

#[cfg(all(
	feature = "installer-test",
	any(feature = "hfsplus-read", feature = "apfs-read"),
	any(target_os = "linux", target_os = "macos")
))]
mod execute;

pub(super) fn run(arguments: &Arguments) -> CliResult<ExitCode> {
	#[cfg(all(
		feature = "installer-test",
		any(feature = "hfsplus-read", feature = "apfs-read"),
		any(target_os = "linux", target_os = "macos")
	))]
	{
		arguments.selection.validate()?;
		match arguments.profile {
			Profile::Installer => execute::run(arguments),
		}
	}
	#[cfg(not(all(
		feature = "installer-test",
		any(feature = "hfsplus-read", feature = "apfs-read"),
		any(target_os = "linux", target_os = "macos")
	)))]
	{
		let _ = arguments;
		Err(format!(
			"installer testing requires {FEATURE} and a composed filesystem reader on macOS or Linux"
		)
		.into())
	}
}
