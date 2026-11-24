//! Discover partition geometry in an immutable logical byte source (D-READ-01).
//!
//! Container block maps translate logical reads. They do not supply partition
//! identity. Filesystem interpretation is performed by the selected engine.

use std::{fmt, num::NonZeroU32, ops::Range};

use dmgez_udif::{
	Error as SourceError,
	reader::{Image, ReadAt},
};
use thiserror::Error;

mod apm;
mod discovery;
mod gpt;
mod mbr;
mod window;

#[cfg(kani)]
#[path = "../verification/kani/window.rs"]
mod window_proof;

pub use window::Window;

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

/// Explicit selection of an admitted whole-disk volume or a partition slot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum VolumeId {
	WholeDisk,
	Partition(PartitionId),
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

/// A recognized whole-disk filesystem header and bounded allocation geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Filesystem {
	HfsPlus,
	HfsX,
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

/// Failure discovering, selecting, or streaming a logical disk extent.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Source(#[from] SourceError),
	#[error("corrupt {scheme} map: {reason}")]
	Corrupt {
		scheme: Scheme,
		reason: &'static str,
	},
	#[error("unsupported {scheme} map: {reason}")]
	Unsupported {
		scheme: Scheme,
		reason: &'static str,
	},
	#[error("partition discovery conflict: {reason}")]
	Conflict { reason: &'static str },
	#[error("resource limit: {0}")]
	Limit(&'static str),
	#[error("logical range {offset} + {len} exceeds {available} bytes")]
	OutOfRange {
		offset: u64,
		len: u64,
		available: u64,
	},
	#[error("volume selector is absent from this disk: {0:?}")]
	UnknownVolume(VolumeId),
	#[error("partition has no admitted filesystem data window: {0:?}")]
	NoDataWindow(PartitionId),
	#[error("no usable volume is available on this disk")]
	NoVolume,
	#[error("{candidates} usable volumes require an explicit selector")]
	AmbiguousVolumes { candidates: usize },
	#[error("a nonempty raw export requires a nonempty copy buffer")]
	EmptyCopyBuffer,
	#[error("raw export cancelled")]
	Cancelled,
	#[error("raw export sink failed")]
	Sink(#[source] std::io::Error),
	#[error("corrupt partitionless HFS volume geometry")]
	VolumeGeometry,
}

/// Byte-addressed logical storage with explicit cache mutability.
///
/// Implementors must preserve the admitted length and reject changed sources,
/// out-of-bounds requests and short reads. Empty reads at EOF are legal and
/// must still validate source identity.
pub trait LogicalDisk {
	fn len(&self) -> u64;
	fn is_empty(&self) -> bool {
		self.len() == 0
	}
	/// Returns source, bounds, integrity, and codec errors without zero filling.
	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError>;
}

impl<S: LogicalDisk + ?Sized> LogicalDisk for &mut S {
	fn len(&self) -> u64 {
		(**self).len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		(**self).read_exact_at(offset, output)
	}
}

impl<S: ReadAt> LogicalDisk for Image<S> {
	fn len(&self) -> u64 {
		self.descriptor().logical_bytes
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		Image::read_exact_at(self, offset, output)
	}
}

/// Explicitly treats a finite source as a raw logical disk.
#[derive(Debug)]
pub struct Raw<S>(pub S);

impl<S: ReadAt> LogicalDisk for Raw<S> {
	fn len(&self) -> u64 {
		self.0.len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		self.0.check_unchanged()?;
		self.0.read_exact_at(offset, output)?;
		self.0.check_unchanged()
	}
}

/// Discovered layout scoped to its admitted logical source.
#[derive(Debug)]
pub struct Disk<S> {
	source: S,
	layout: Layout,
}

impl<S: LogicalDisk> Disk<S> {
	/// Reads interpretation-relevant map bytes under the configured limits.
	///
	/// # Errors
	/// Rejects unsupported, conflicting, or malformed map metadata and failed reads.
	pub fn discover(mut source: S, limits: Limits) -> Result<Self, Error> {
		let layout = discovery::discover(&mut source, limits)?;
		Ok(Self { source, layout })
	}

	#[must_use]
	pub const fn layout(&self) -> &Layout {
		&self.layout
	}

	/// Resolve a requested filesystem data extent, or the only eligible extent.
	///
	/// # Errors
	/// Rejects missing selectors, metadata-only entries, and automatic selection
	/// with zero or multiple eligible extents.
	pub fn select_volume(&self, requested: Option<VolumeId>) -> Result<VolumeId, Error> {
		if let Some(id) = requested {
			self.volume_data_range(id)?;
			return Ok(id);
		}
		match &self.layout {
			Layout::Partitionless { .. } => Ok(VolumeId::WholeDisk),
			Layout::Partitioned { partitions, .. } => {
				let mut eligible = partitions.iter().filter(|entry| entry.data_range.is_some());
				let first = eligible.next().ok_or(Error::NoVolume)?;
				let candidates = 1 + eligible.count();
				if candidates != 1 {
					return Err(Error::AmbiguousVolumes { candidates });
				}
				Ok(VolumeId::Partition(first.id))
			}
			Layout::Unknown => Err(Error::NoVolume),
		}
	}

	/// Return a window over the complete partition or admitted partitionless volume.
	///
	/// # Errors
	/// Fails if the requested record does not exist in this disk view.
	pub fn window(&mut self, id: VolumeId) -> Result<Window<'_, S>, Error> {
		let range = match &self.layout {
			Layout::Partitionless { range, .. } if id == VolumeId::WholeDisk => range.clone(),
			_ => self.partition(id)?.range.clone(),
		};
		Ok(Window::new(&mut self.source, range))
	}

	/// Return the admitted filesystem data extent inside the selected record.
	///
	/// # Errors
	/// Fails if the requested record is missing or has no filesystem data window.
	pub fn data_window(&mut self, id: VolumeId) -> Result<Window<'_, S>, Error> {
		let range = self.volume_data_range(id)?.clone();
		Ok(Window::new(&mut self.source, range))
	}

	fn volume_data_range(&self, id: VolumeId) -> Result<&Range<u64>, Error> {
		match &self.layout {
			Layout::Partitionless { range, .. } if id == VolumeId::WholeDisk => Ok(range),
			_ => {
				let partition = self.partition(id)?;
				partition
					.data_range
					.as_ref()
					.ok_or(Error::NoDataWindow(partition.id))
			}
		}
	}

	fn partition(&self, id: VolumeId) -> Result<&Partition, Error> {
		if let (Layout::Partitioned { partitions, .. }, VolumeId::Partition(id)) =
			(&self.layout, id)
			&& let Some(partition) = partitions.iter().find(|entry| entry.id == id)
		{
			return Ok(partition);
		}
		Err(Error::UnknownVolume(id))
	}
}
