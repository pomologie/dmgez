use std::path::PathBuf;

use clap::Args;

use crate::{
	CliResult,
	volume_commands::{Filesystem, InputArguments, Selection},
};

pub(super) const HFSPLUS_FEATURE: &str = "hfsplus-install";
pub(super) const APFS_FEATURE: &str = "apfs-install";
pub(super) const COMMAND: &str = "install";
pub(super) const HFSPLUS_COMPILED: bool = cfg!(all(
	feature = "hfsplus-install",
	any(target_os = "linux", target_os = "macos")
));
pub(super) const APFS_COMPILED: bool = cfg!(all(
	feature = "apfs-install",
	any(target_os = "linux", target_os = "macos")
));

const BYTE_COUNT_VALUE: &str = "BYTES";
const MAX_BYTES_OPTION: &str = "max-bytes";
const NATIVE_POLICY_FEATURE: &str = "signing-native";

#[derive(Args)]
pub(super) struct Arguments {
	#[command(flatten)]
	input: InputArguments,
	#[command(flatten)]
	filesystem: Selection,
	/// Absolute path of one application bundle inside the selected volume.
	#[arg(long)]
	app: String,
	/// Existing directory where the bundle will be copied under its stored name.
	#[arg(long)]
	destination: PathBuf,
	/// Check the bundle and destination without creating any files.
	#[arg(long)]
	dry_run: bool,
	/// Require native Developer ID Application verification of the staged bundle.
	#[arg(long)]
	verify_app_signature: bool,
	/// Require Gatekeeper acceptance of the staged bundle.
	#[arg(long)]
	assess: bool,
	/// Lower the limit for data, resource forks, attributes, and link targets.
	#[arg(long = MAX_BYTES_OPTION, value_name = BYTE_COUNT_VALUE)]
	max_bytes: Option<u64>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[cfg(all(
	any(feature = "hfsplus-install", feature = "apfs-install"),
	any(target_os = "linux", target_os = "macos")
))]
mod execute;

pub(super) fn run(arguments: &Arguments) -> CliResult {
	arguments.filesystem.validate()?;
	if (arguments.verify_app_signature || arguments.assess)
		&& !cfg!(all(feature = "signing-native", target_os = "macos"))
	{
		return Err(format!(
			"application signature verification and Gatekeeper assessment require macOS and the {NATIVE_POLICY_FEATURE} feature"
		)
		.into());
	}
	match arguments.filesystem.filesystem() {
		Filesystem::Hfsplus => {
			#[cfg(all(
				feature = "hfsplus-install",
				any(target_os = "linux", target_os = "macos")
			))]
			{
				execute::hfsplus(arguments)
			}
			#[cfg(not(all(
				feature = "hfsplus-install",
				any(target_os = "linux", target_os = "macos")
			)))]
			{
				Err(unavailable(
					HFSPLUS_FEATURE,
					cfg!(feature = "hfsplus-install"),
				))
			}
		}
		Filesystem::Apfs => {
			#[cfg(all(
				feature = "apfs-install",
				any(target_os = "linux", target_os = "macos")
			))]
			{
				execute::apfs(arguments)
			}
			#[cfg(not(all(
				feature = "apfs-install",
				any(target_os = "linux", target_os = "macos")
			)))]
			{
				Err(unavailable(APFS_FEATURE, cfg!(feature = "apfs-install")))
			}
		}
	}
}

#[cfg(not(all(
	feature = "hfsplus-install",
	feature = "apfs-install",
	any(target_os = "linux", target_os = "macos")
)))]
fn unavailable(feature: &'static str, compiled: bool) -> Box<dyn std::error::Error> {
	if compiled {
		return "mountless application copying requires a macOS or Linux host".into();
	}
	format!("feature not compiled: {feature}").into()
}
