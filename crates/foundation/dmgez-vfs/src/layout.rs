use std::ops::Range;

use crate::{Partition, Scheme};

/// A recognized whole-disk filesystem header and bounded allocation geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Filesystem {
	HfsPlus,
	HfsX,
	Apfs,
}

/// Discovered partition geometry. Filesystem contents require separate checks.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Layout {
	Partitioned {
		scheme: Scheme,
		block_bytes: u64,
		partitions: Vec<Partition>,
	},
	Partitionless {
		filesystem: Filesystem,
		range: Range<u64>,
	},
	Unknown,
}

/// Limits on partition table bytes and record traversal.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	/// Maximum entry count (including empty GPT slots) or EBR chain length.
	pub max_entries: u32,
	/// Maximum combined input bytes read through partition discovery.
	pub max_probe_bytes: u64,
}

const DEFAULT_MAX_ENTRIES: u32 = 1024;
const DEFAULT_PROBE_BYTES: u64 = 1 << 20;

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_entries: DEFAULT_MAX_ENTRIES,
			max_probe_bytes: DEFAULT_PROBE_BYTES,
		}
	}
}
