use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::{
	CliResult,
	volume_commands::{Filesystem, InputArguments, Selection},
};

pub(super) const HFSPLUS_FEATURE: &str = "hfsplus-extract";
pub(super) const HFSPLUS_COMPILED: bool = cfg!(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
));
pub(super) const APFS_FEATURE: &str = "apfs-extract";
pub(super) const APFS_COMPILED: bool = cfg!(all(
	feature = "apfs-extract",
	any(target_os = "linux", target_os = "macos")
));

const PROJECTION_GROUP: &str = "projection";
const MAX_BYTES_OPTION: &str = "max-bytes";
const BYTE_COUNT_VALUE_NAME: &str = "BYTES";

#[derive(Args)]
pub(super) struct Arguments {
	#[command(flatten)]
	input: InputArguments,
	#[command(flatten)]
	filesystem: Selection,
	/// Absolute source path inside the selected volume.
	path: String,
	/// An absent destination file or directory under an existing parent.
	#[arg(short, long)]
	output: PathBuf,
	/// Plan and check the destination without creating files or directories.
	#[arg(long)]
	dry_run: bool,
	/// Lower the total payload byte limit. Zero allows only an empty projection.
	#[arg(long = MAX_BYTES_OPTION, value_name = BYTE_COUNT_VALUE_NAME)]
	max_bytes: Option<u64>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
	#[command(flatten)]
	projection: Projection,
}

#[derive(Args)]
#[group(id = PROJECTION_GROUP, required = true, multiple = false)]
struct Projection {
	/// Export one file's selected fork as a separate host file.
	#[arg(long, value_enum)]
	fork: Option<Fork>,
	/// Export directory topology and file data with the reported metadata omissions.
	#[arg(long)]
	data_only: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Fork {
	Data,
	Resource,
}

#[cfg(all(
	any(feature = "hfsplus-extract", feature = "apfs-extract"),
	any(target_os = "linux", target_os = "macos")
))]
mod execute;

#[cfg(not(all(
	feature = "hfsplus-extract",
	feature = "apfs-extract",
	any(target_os = "linux", target_os = "macos")
)))]
mod unavailable;

pub(super) fn run(arguments: &Arguments) -> CliResult {
	arguments.filesystem.validate()?;
	match arguments.filesystem.filesystem() {
		Filesystem::Hfsplus => {
			#[cfg(all(
				feature = "hfsplus-extract",
				any(target_os = "linux", target_os = "macos")
			))]
			{
				execute::hfsplus(arguments)
			}
			#[cfg(not(all(
				feature = "hfsplus-extract",
				any(target_os = "linux", target_os = "macos")
			)))]
			{
				Err(
					unavailable::Error::new(HFSPLUS_FEATURE, cfg!(feature = "hfsplus-extract"))
						.into(),
				)
			}
		}
		Filesystem::Apfs => {
			#[cfg(all(
				feature = "apfs-extract",
				any(target_os = "linux", target_os = "macos")
			))]
			{
				execute::apfs(arguments)
			}
			#[cfg(not(all(
				feature = "apfs-extract",
				any(target_os = "linux", target_os = "macos")
			)))]
			{
				Err(unavailable::Error::new(APFS_FEATURE, cfg!(feature = "apfs-extract")).into())
			}
		}
	}
}
