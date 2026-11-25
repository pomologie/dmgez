use std::{
	num::{NonZeroU32, NonZeroU64},
	path::PathBuf,
};

use clap::Args;

use crate::volume_commands::{PARTITION_SLOT_VALUE_NAME, Selection};

#[cfg(feature = "inspect")]
use std::io;

#[cfg(feature = "inspect")]
use dmgez_runtime::inspection::{MAX_FILESYSTEM_PAYLOAD_BYTES, PayloadBudget};

#[cfg(feature = "inspect")]
use crate::{
	CliResult, inspect,
	volume_commands::{APFS_FILESYSTEM_VALUE, FILESYSTEM_OPTION, Filesystem},
};

const FULL_ARG: &str = "full";
const PAYLOAD_BYTES_OPTION: &str = "max-payload-bytes";

#[derive(Args)]
pub(super) struct Arguments {
	pub(super) image: PathBuf,
	/// Write a versioned JSON report.
	#[arg(long)]
	pub(super) json: bool,
	#[command(flatten)]
	scope: Scope,
	#[command(flatten)]
	pub(super) selection: Selection,
	/// Interpret the input as a raw APFS disk image.
	#[arg(long, requires = FULL_ARG)]
	pub(super) raw: bool,
	/// Select a one-based partition slot for filesystem verification.
	#[arg(long, requires = FULL_ARG, value_name = PARTITION_SLOT_VALUE_NAME)]
	pub(super) partition: Option<NonZeroU32>,
	/// Limit HFS+ stored or APFS logical payload reads to this many bytes, up to 1 GiB.
	#[arg(long = PAYLOAD_BYTES_OPTION, requires = FULL_ARG)]
	max_payload_bytes: Option<NonZeroU64>,
}

#[derive(Args)]
#[group(multiple = false)]
struct Scope {
	/// Check container structure without reading payloads.
	#[arg(long)]
	quick: bool,
	/// Check structure, payload, and all declared container CRCs. This is the default.
	#[arg(long)]
	container: bool,
	/// Check the filesystem and, for UDIF images, the container.
	#[arg(long, id = FULL_ARG)]
	full: bool,
}

#[cfg(feature = "inspect")]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	use dmgez_inspector::VerificationMode;

	let mode = match arguments.scope {
		Scope { quick: true, .. } => VerificationMode::Quick,
		Scope { full: true, .. } => VerificationMode::Full,
		_ => VerificationMode::Container,
	};
	if mode != VerificationMode::Full && arguments.selection.has_options() {
		return Err(invalid(format!("filesystem selection requires --{FULL_ARG}")).into());
	}
	arguments.selection.validate()?;
	if arguments.raw && matches!(arguments.selection.filesystem(), Filesystem::Hfsplus) {
		return Err(invalid(format!(
			"--raw requires --{FILESYSTEM_OPTION} {APFS_FILESYSTEM_VALUE}"
		))
		.into());
	}
	let budget = match arguments.max_payload_bytes {
		Some(bytes) => PayloadBudget::new(bytes).ok_or_else(|| {
			invalid(format!(
				"--{PAYLOAD_BYTES_OPTION} must be at most {MAX_FILESYSTEM_PAYLOAD_BYTES}"
			))
		})?,
		None => PayloadBudget::default(),
	};
	inspect::verify(arguments, mode, budget)
}

#[cfg(feature = "inspect")]
fn invalid(message: String) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidInput, message)
}
