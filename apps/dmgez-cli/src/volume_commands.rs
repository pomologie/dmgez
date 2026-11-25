use std::{
	io,
	num::{NonZeroU32, NonZeroU64},
	path::PathBuf,
};

use clap::{Args, ValueEnum};

use crate::CliResult;

#[cfg(feature = "apfs-read")]
mod apfs;
#[cfg(any(
	all(feature = "diff", feature = "apfs-read"),
	all(
		feature = "installer-test",
		feature = "apfs-read",
		any(target_os = "linux", target_os = "macos")
	),
	all(
		any(feature = "apfs-extract", feature = "apfs-install"),
		any(target_os = "linux", target_os = "macos")
	)
))]
pub(super) use apfs::VolumeIdentity;
#[cfg(all(
	any(feature = "apfs-extract", feature = "apfs-install"),
	any(target_os = "linux", target_os = "macos")
))]
pub(super) use apfs::with_selector_guidance;
#[cfg(all(feature = "diff", feature = "apfs-read"))]
pub(super) use apfs::{checkpoint, volume_selector, with_named_selector_guidance};
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod execute;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod report;

pub(super) const HFSPLUS_READ_FEATURE: &str = "hfsplus-read";
pub(super) const APFS_READ_FEATURE: &str = "apfs-read";
pub(super) const VOLUMES_COMMAND: &str = "volumes";

const ROOT_PATH: &str = "/";
pub(super) const PARTITION_SLOT_VALUE_NAME: &str = "SLOT";
pub(super) const FILESYSTEM_OPTION: &str = "filesystem";
pub(super) const HFSPLUS_FILESYSTEM_VALUE: &str = "hfsplus";
pub(super) const APFS_FILESYSTEM_VALUE: &str = "apfs";
pub(super) const VOLUME_VALUE_NAME: &str = "UUID|OID";
const CHECKPOINT_VALUE_NAME: &str = "XID";
pub(super) const VOLUME_OPTION: &str = "volume";
const CHECKPOINT_OPTION: &str = "xid";

#[derive(Args)]
pub(super) struct InputArguments {
	/// UDIF image file, or a regular raw disk image with --raw.
	pub(super) image: PathBuf,
	/// Read a raw disk image instead of a UDIF container.
	#[arg(long)]
	pub(super) raw: bool,
	/// Select a one-based partition slot. Otherwise one data window is required.
	#[arg(long, value_name = PARTITION_SLOT_VALUE_NAME)]
	pub(super) partition: Option<NonZeroU32>,
}

#[derive(Args)]
pub(super) struct Selection {
	/// Select the filesystem. Defaults to HFS+, which also recognizes HFSX.
	#[arg(long = FILESYSTEM_OPTION, value_enum)]
	filesystem: Option<Filesystem>,
	/// Select an APFS volume by UUID or decimal object ID.
	#[arg(long = VOLUME_OPTION, value_name = VOLUME_VALUE_NAME)]
	volume: Option<String>,
	/// Select an exact retained APFS container checkpoint by decimal transaction ID.
	#[arg(long = CHECKPOINT_OPTION, value_name = CHECKPOINT_VALUE_NAME)]
	xid: Option<NonZeroU64>,
}

#[derive(Clone, Copy, Default, ValueEnum)]
pub(super) enum Filesystem {
	#[default]
	#[value(name = HFSPLUS_FILESYSTEM_VALUE)]
	Hfsplus,
	#[value(name = APFS_FILESYSTEM_VALUE)]
	Apfs,
}

#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
impl Filesystem {
	pub(super) const fn as_str(self) -> &'static str {
		match self {
			Self::Hfsplus => HFSPLUS_FILESYSTEM_VALUE,
			Self::Apfs => APFS_FILESYSTEM_VALUE,
		}
	}
}

impl Selection {
	pub(super) fn filesystem(&self) -> Filesystem {
		self.filesystem.unwrap_or_default()
	}

	#[cfg(feature = "inspect")]
	pub(super) fn has_options(&self) -> bool {
		self.filesystem.is_some() || self.volume.is_some() || self.xid.is_some()
	}

	pub(super) fn validate(&self) -> io::Result<()> {
		if matches!(self.filesystem(), Filesystem::Hfsplus)
			&& (self.volume.is_some() || self.xid.is_some())
		{
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!(
					"--{VOLUME_OPTION} and --{CHECKPOINT_OPTION} require --{FILESYSTEM_OPTION} {APFS_FILESYSTEM_VALUE}"
				),
			));
		}
		Ok(())
	}

	#[cfg(feature = "apfs-read")]
	pub(super) fn apfs_volume(&self) -> io::Result<Option<dmgez_apfs_read::VolumeSelector>> {
		self.volume
			.as_deref()
			.map(apfs::volume_selector)
			.transpose()
	}

	#[cfg(feature = "apfs-read")]
	pub(super) fn apfs_checkpoint(&self) -> Option<dmgez_apfs_read::TransactionId> {
		apfs::checkpoint(self.xid)
	}
}

#[derive(Args)]
pub(super) struct ListArguments {
	#[command(flatten)]
	input: InputArguments,
	#[command(flatten)]
	selection: Selection,
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
	#[command(flatten)]
	selection: Selection,
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
	#[command(flatten)]
	selection: Selection,
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
	#[command(flatten)]
	selection: Selection,
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

#[derive(Args)]
pub(super) struct VolumesArguments {
	#[command(flatten)]
	input: InputArguments,
	/// List an exact retained APFS container checkpoint by decimal transaction ID.
	#[arg(long = CHECKPOINT_OPTION, value_name = CHECKPOINT_VALUE_NAME)]
	xid: Option<NonZeroU64>,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

pub(super) enum Query<'args> {
	List(&'args ListArguments),
	Tree(&'args TreeArguments),
	Stat(&'args StatArguments),
	Cat(&'args CatArguments),
}

impl Query<'_> {
	fn selection(&self) -> &Selection {
		match self {
			Self::List(args) => &args.selection,
			Self::Tree(args) => &args.selection,
			Self::Stat(args) => &args.selection,
			Self::Cat(args) => &args.selection,
		}
	}
}

pub(super) fn run(query: Query<'_>) -> CliResult {
	let selection = query.selection();
	selection.validate()?;
	match selection.filesystem() {
		Filesystem::Hfsplus => {
			#[cfg(feature = "hfsplus-read")]
			{
				execute::hfsplus(&query)
			}
			#[cfg(not(feature = "hfsplus-read"))]
			{
				Err(unavailable(HFSPLUS_READ_FEATURE))
			}
		}
		Filesystem::Apfs => {
			#[cfg(feature = "apfs-read")]
			{
				execute::apfs(&query)
			}
			#[cfg(not(feature = "apfs-read"))]
			{
				Err(unavailable(APFS_READ_FEATURE))
			}
		}
	}
}

#[cfg(feature = "apfs-read")]
pub(super) fn volumes(arguments: &VolumesArguments) -> CliResult {
	apfs::volumes(arguments)
}

#[cfg(not(feature = "apfs-read"))]
pub(super) fn volumes(_arguments: &VolumesArguments) -> CliResult {
	Err(unavailable(APFS_READ_FEATURE))
}

#[cfg(not(all(feature = "hfsplus-read", feature = "apfs-read")))]
fn unavailable(feature: &str) -> Box<dyn std::error::Error> {
	format!("feature not compiled: {feature}").into()
}
