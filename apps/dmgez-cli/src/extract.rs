use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::{CliResult, volume_commands::InputArguments};

pub(super) const FEATURE: &str = "hfsplus-extract";
pub(super) const COMPILED: bool = cfg!(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
));

const PROJECTION_GROUP: &str = "projection";
const MAX_BYTES_OPTION: &str = "max-bytes";
const BYTE_COUNT_VALUE_NAME: &str = "BYTES";

#[derive(Args)]
pub(super) struct Arguments {
	#[command(flatten)]
	input: InputArguments,
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
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
mod execute;

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	execute::run(arguments)
}

#[cfg(not(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
)))]
mod unavailable;

#[cfg(not(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
)))]
pub(super) fn run(_arguments: &Arguments) -> CliResult {
	Err(Box::new(unavailable::Error))
}
