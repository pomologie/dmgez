use std::{num::NonZeroU32, path::PathBuf};

use clap::{Args, ValueEnum};

#[cfg(feature = "hfsplus-read")]
mod execute;

#[cfg(feature = "hfsplus-read")]
pub(super) use execute::{cat, list, stat, tree};

pub(super) const FEATURE: &str = "hfsplus-read";

const ROOT_PATH: &str = "/";
const PARTITION_SLOT_VALUE_NAME: &str = "SLOT";

#[derive(Args)]
struct InputArguments {
	/// UDIF image file, or a regular raw disk image with --raw.
	image: PathBuf,
	/// Read a raw disk image instead of a UDIF container.
	#[arg(long)]
	raw: bool,
	/// Select a one-based partition slot. Otherwise one data window is required.
	#[arg(long, value_name = PARTITION_SLOT_VALUE_NAME)]
	partition: Option<NonZeroU32>,
}

#[derive(Args)]
pub(super) struct ListArguments {
	#[command(flatten)]
	input: InputArguments,
	/// Absolute directory path inside the selected volume.
	#[arg(default_value = ROOT_PATH)]
	path: String,
	/// Number of entries to skip in native catalog order.
	#[arg(long, default_value_t)]
	offset: u64,
	/// Maximum entries to report in this page.
	#[arg(long)]
	limit: Option<u16>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[derive(Args)]
pub(super) struct TreeArguments {
	#[command(flatten)]
	input: InputArguments,
	/// Absolute directory path inside the selected volume.
	#[arg(default_value = ROOT_PATH)]
	path: String,
	/// Maximum depth below the selected directory.
	#[arg(long)]
	depth: Option<u16>,
	/// Maximum entries to report in the tree.
	#[arg(long)]
	limit: Option<u32>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[derive(Args)]
pub(super) struct StatArguments {
	#[command(flatten)]
	input: InputArguments,
	/// Absolute path inside the selected volume.
	path: String,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[derive(Args)]
pub(super) struct CatArguments {
	#[command(flatten)]
	input: InputArguments,
	/// Absolute path to a regular file inside the selected volume.
	path: String,
	/// Select the logical data stream or the stored resource fork.
	#[arg(long, value_enum, default_value_t)]
	fork: Fork,
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum Fork {
	#[default]
	Data,
	Resource,
}
