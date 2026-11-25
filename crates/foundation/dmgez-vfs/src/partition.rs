use std::{fmt, num::NonZeroU32, ops::Range};

/// The partition table format and corresponding slot namespace.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Scheme {
	Apple,
	Gpt,
	Mbr,
}

impl fmt::Display for Scheme {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Apple => "APM",
			Self::Gpt => "GPT",
			Self::Mbr => "MBR",
		})
	}
}

/// One table entry in an opened logical disk. Slots start at one.
/// IDs are meaningful only in the disk view that produced them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PartitionId {
	pub scheme: Scheme,
	pub slot: NonZeroU32,
}

/// The partition's relationship to the rest of the map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PartitionRole {
	Data,
	Free,
	Metadata,
	ExtendedContainer,
}

/// Format-specific report fields. GUIDs use standard UUID byte order.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PartitionKind {
	Apple {
		name: String,
		type_code: String,
		status: u32,
	},
	Gpt {
		name: String,
		type_guid: [u8; 16],
		unique_guid: [u8; 16],
		attributes: u64,
	},
	Mbr {
		type_code: u8,
		bootable: bool,
	},
}

/// A record in an admitted map. Ranges use logical disk byte offsets.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Partition {
	pub id: PartitionId,
	pub role: PartitionRole,
	/// Complete physical allocation on the decoded logical disk.
	pub range: Range<u64>,
	/// Usable file-data window, accounting for any declared APM data subrange.
	pub data_range: Option<Range<u64>>,
	pub kind: PartitionKind,
}
