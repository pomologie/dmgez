//! A bounded logical disk envelope around one sealed HFS+ volume.

use std::io::Read;

use dmgez_udif::{SECTOR_BYTES, writer::Cancellation};
use thiserror::Error;

#[cfg(feature = "partition-write")]
mod apm;
#[cfg(feature = "partition-write")]
mod gpt;

const ALLOCATION_BLOCK_BYTES: u64 = 4096;
const APM_HEAD_SECTORS: u64 = 64;
const GPT_EDGE_SECTORS: u64 = 40;
// Covers the retained maps and either map serialization or volume hashing.
const MAP_WORK_BYTES: usize = 64 << 10;
#[cfg(feature = "partition-write")]
const VOLUME_PARTITION_NAME: &str = "disk image";

/// The map profile around the existing filesystem volume.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Layout {
	#[default]
	Partitionless,
	Apm,
	Gpt,
}

impl Layout {
	pub(crate) const fn work_bytes(self) -> usize {
		match self {
			Self::Partitionless => 0,
			Self::Apm | Self::Gpt => MAP_WORK_BYTES,
		}
	}

	/// Logical bytes occupied by the map and its alignment padding.
	#[must_use]
	pub const fn overhead_bytes(self) -> u64 {
		match self {
			Self::Partitionless => 0,
			Self::Apm => APM_HEAD_SECTORS * SECTOR_BYTES,
			Self::Gpt => 2 * GPT_EDGE_SECTORS * SECTOR_BYTES,
		}
	}

	/// Maximum outer disk extent representable by the selected layout.
	#[must_use]
	pub const fn max_disk_bytes(self) -> u64 {
		match self {
			Self::Apm => u32::MAX as u64 * SECTOR_BYTES,
			_ => u64::MAX,
		}
	}

	/// Exact logical disk length, including map reservations.
	#[must_use]
	pub fn disk_bytes(self, volume_bytes: u64) -> Option<u64> {
		if self != Self::Partitionless
			&& (volume_bytes == 0 || !volume_bytes.is_multiple_of(ALLOCATION_BLOCK_BYTES))
		{
			return None;
		}
		volume_bytes
			.checked_add(self.overhead_bytes())
			.filter(|&bytes| bytes <= self.max_disk_bytes())
	}
}

/// A sealed prefix and suffix. No volume payload is retained in this plan.
#[derive(Debug)]
pub struct Envelope {
	prefix: Vec<u8>,
	suffix: Vec<u8>,
}

impl Envelope {
	/// Prepare the exact map bytes for a sealed volume. GPT obtains its deterministic
	/// identifiers from the sealed volume stream. Other layouts do not open it.
	///
	/// # Errors
	/// Rejects an unsupported layout, invalid size, cancellation, or failed map generation.
	pub fn from_volume<R: Read>(
		layout: Layout,
		volume_bytes: u64,
		open_volume: impl FnOnce() -> R,
		cancellation: &Cancellation,
	) -> Result<Self, Error> {
		let disk_bytes = layout.disk_bytes(volume_bytes).ok_or(Error::Geometry)?;
		match layout {
			Layout::Partitionless => Ok(Self {
				prefix: Vec::new(),
				suffix: Vec::new(),
			}),
			#[cfg(feature = "partition-write")]
			Layout::Apm => apm::generate(volume_bytes, disk_bytes),
			#[cfg(feature = "partition-write")]
			Layout::Gpt => gpt::generate(volume_bytes, disk_bytes, open_volume(), cancellation),
			#[cfg(not(feature = "partition-write"))]
			_ => {
				let _ = (disk_bytes, open_volume, cancellation);
				Err(Error::Unavailable)
			}
		}
	}

	/// Stream the map, the same sealed HFS+ volume, and the backup map if present.
	pub fn reader<'plan, R: Read + 'plan>(&'plan self, volume: R) -> impl Read + 'plan {
		self.prefix
			.as_slice()
			.chain(volume)
			.chain(self.suffix.as_slice())
	}
}

/// Map generation or geometry failure before publication.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("partition layout is outside the supported logical disk geometry")]
	Geometry,
	#[error("partition-write is unavailable")]
	Unavailable,
	#[error("partition map generation was cancelled")]
	Cancelled,
	#[error("could not hash the sealed HFS+ volume for GPT identities")]
	HashVolume(#[source] std::io::Error),
	#[error("could not serialize the selected partition map")]
	Generate(#[source] Box<dyn std::error::Error + Send + Sync>),
}
