use std::{
	num::{NonZeroU32, NonZeroU64},
	path::PathBuf,
	process::ExitCode,
};

use clap::{Args, ValueEnum};

#[cfg(feature = "diff")]
use dmgez_diff::Side;
#[cfg(feature = "diff")]
use std::{error::Error as StdError, fmt, path::Path};

use crate::{
	CliResult,
	volume_commands::{
		FILESYSTEM_OPTION, Filesystem, PARTITION_SLOT_VALUE_NAME, VOLUME_VALUE_NAME,
	},
};

#[cfg(feature = "diff")]
mod bytes;
#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
mod semantic;

pub(super) const COMMAND: &str = "diff";
pub(super) const FINDER_DIFF_FEATURE: &str = "finder-diff";
pub(super) const DIFF_HASH_FEATURE: &str = "diff-hash";
pub(super) const HFSPLUS_CAPABILITY: &str = "diff-hfsplus";
pub(super) const APFS_CAPABILITY: &str = "diff-apfs";
pub(super) const HFSPLUS_COMPILED: bool = cfg!(all(feature = "diff", feature = "hfsplus-read"));
pub(super) const APFS_COMPILED: bool = cfg!(all(feature = "diff", feature = "apfs-read"));

#[cfg(feature = "diff")]
const DIFFERENT_EXIT_CODE: u8 = 1;
const XID_VALUE_NAME: &str = "XID";
const LEFT_VOLUME_OPTION: &str = "left-volume";
const RIGHT_VOLUME_OPTION: &str = "right-volume";

#[derive(Args)]
pub(super) struct Arguments {
	left: PathBuf,
	right: PathBuf,
	#[arg(long, value_enum, default_value_t)]
	mode: Mode,
	/// Maximum bytes charged to the comparison budget.
	#[arg(long)]
	max_bytes: Option<u64>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
	#[command(flatten)]
	options: SemanticOptions,
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum Mode {
	#[default]
	Logical,
	Physical,
	Semantic,
}

#[derive(Clone, Copy, Default, ValueEnum)]
enum ContentsMode {
	#[default]
	Exact,
	Blake3,
	Blake3Exact,
}

#[derive(Args)]
struct SemanticOptions {
	/// Select a filesystem for both inputs in semantic mode. HFS+ includes HFSX.
	#[arg(long = FILESYSTEM_OPTION, value_enum)]
	filesystem: Option<Filesystem>,
	/// Compare regular-file forks by exact bytes (default), BLAKE3, or BLAKE3 with exact confirmation.
	#[arg(long, value_enum)]
	contents: Option<ContentsMode>,
	/// Report unique rename candidates among otherwise added and removed files.
	#[arg(long)]
	infer_renames: bool,
	/// Include supported extended attributes in the semantic comparison.
	#[arg(long)]
	xattrs: bool,
	/// Compare effective per-object FinderInfo in semantic mode.
	#[arg(long)]
	finder_info: bool,
	/// Compare supported Finder settings from matched .DS_Store files.
	#[arg(long)]
	finder_layout: bool,
	/// Interpret the left input as a raw disk image in semantic mode.
	#[arg(long)]
	left_raw: bool,
	/// Interpret the right input as a raw disk image in semantic mode.
	#[arg(long)]
	right_raw: bool,
	/// Select a one-based partition slot in the left disk image.
	#[arg(long, value_name = PARTITION_SLOT_VALUE_NAME)]
	left_partition: Option<NonZeroU32>,
	/// Select a one-based partition slot in the right disk image.
	#[arg(long, value_name = PARTITION_SLOT_VALUE_NAME)]
	right_partition: Option<NonZeroU32>,
	/// Select the left APFS volume by UUID or decimal object ID.
	#[arg(long = LEFT_VOLUME_OPTION, value_name = VOLUME_VALUE_NAME)]
	left_volume: Option<String>,
	/// Select the right APFS volume by UUID or decimal object ID.
	#[arg(long = RIGHT_VOLUME_OPTION, value_name = VOLUME_VALUE_NAME)]
	right_volume: Option<String>,
	/// Select an exact retained APFS checkpoint in the left image.
	#[arg(long, value_name = XID_VALUE_NAME)]
	left_xid: Option<NonZeroU64>,
	/// Select an exact retained APFS checkpoint in the right image.
	#[arg(long, value_name = XID_VALUE_NAME)]
	right_xid: Option<NonZeroU64>,
}

#[cfg(feature = "diff")]
impl SemanticOptions {
	fn has_selectors(&self) -> bool {
		self.filesystem.is_some()
			|| self.left_raw
			|| self.right_raw
			|| self.left_partition.is_some()
			|| self.right_partition.is_some()
			|| self.has_apfs_selectors()
	}

	fn has_apfs_selectors(&self) -> bool {
		self.left_volume.is_some()
			|| self.right_volume.is_some()
			|| self.left_xid.is_some()
			|| self.right_xid.is_some()
	}
}

#[cfg(not(feature = "diff"))]
pub(super) fn run(_arguments: &Arguments) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {COMMAND}").into())
}

#[cfg(feature = "diff")]
pub(super) fn run(arguments: &Arguments) -> CliResult<ExitCode> {
	match arguments.mode {
		Mode::Semantic => run_semantic(arguments),
		_ if arguments.options.contents.is_some() => {
			Err(invalid("--contents requires --mode semantic").into())
		}
		_ if arguments.options.infer_renames => {
			Err(invalid("--infer-renames requires --mode semantic").into())
		}
		_ if arguments.options.xattrs => Err(invalid("--xattrs requires --mode semantic").into()),
		_ if arguments.options.finder_info => {
			Err(invalid("--finder-info requires --mode semantic").into())
		}
		_ if arguments.options.finder_layout => {
			Err(invalid("--finder-layout requires --mode semantic").into())
		}
		_ if arguments.options.has_selectors() => Err(invalid(
			"filesystem, raw input, partition, volume and checkpoint selectors require --mode semantic",
		)
		.into()),
		mode => bytes::run(arguments, matches!(mode, Mode::Logical)),
	}
}

#[cfg(feature = "diff")]
fn run_semantic(arguments: &Arguments) -> CliResult<ExitCode> {
	let filesystem = arguments
		.options
		.filesystem
		.ok_or_else(|| invalid("semantic comparison requires --filesystem hfsplus or apfs"))?;
	#[cfg(not(feature = "finder-diff"))]
	if arguments.options.finder_layout {
		return Err(format!("feature not compiled: {FINDER_DIFF_FEATURE}").into());
	}
	#[cfg(not(feature = "diff-hash"))]
	if arguments.options.infer_renames
		|| arguments
			.options
			.contents
			.is_some_and(|mode| !matches!(mode, ContentsMode::Exact))
	{
		return Err(std::io::Error::new(
			std::io::ErrorKind::Unsupported,
			format!("feature not compiled: {DIFF_HASH_FEATURE}"),
		)
		.into());
	}
	match filesystem {
		Filesystem::Hfsplus => {
			if arguments.options.has_apfs_selectors() {
				return Err(
					invalid("volume and checkpoint selectors require --filesystem apfs").into(),
				);
			}
			#[cfg(feature = "hfsplus-read")]
			return semantic::hfsplus(arguments);
			#[cfg(not(feature = "hfsplus-read"))]
			missing_backend(crate::volume_commands::HFSPLUS_READ_FEATURE)
		}
		Filesystem::Apfs => {
			#[cfg(feature = "apfs-read")]
			return semantic::apfs(arguments);
			#[cfg(not(feature = "apfs-read"))]
			missing_backend(crate::volume_commands::APFS_READ_FEATURE)
		}
	}
}

#[cfg(all(
	feature = "diff",
	any(not(feature = "hfsplus-read"), not(feature = "apfs-read"))
))]
fn missing_backend(feature: &str) -> CliResult<ExitCode> {
	Err(format!("semantic comparison requires the composed {feature} profile").into())
}

#[cfg(feature = "diff")]
fn invalid(message: &'static str) -> std::io::Error {
	std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}

#[cfg(feature = "diff")]
fn require_regular_file(path: &Path) -> Result<(), dmgez_udif::Error> {
	if !path.metadata()?.is_file() {
		return Err(dmgez_udif::Error::InvalidFormat(
			"comparison input is not a regular file",
		));
	}
	Ok(())
}

#[cfg(feature = "diff")]
fn with_side<T, E: Into<Box<dyn StdError>>>(
	result: Result<T, E>,
	side: Side,
) -> Result<T, InputError> {
	result.map_err(|error| InputError {
		side,
		error: error.into(),
	})
}

#[cfg(feature = "diff")]
#[derive(Debug)]
struct InputError {
	side: Side,
	error: Box<dyn StdError>,
}

#[cfg(feature = "diff")]
impl fmt::Display for InputError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "could not admit the {} input", self.side)
	}
}

#[cfg(feature = "diff")]
impl StdError for InputError {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		Some(self.error.as_ref())
	}
}
