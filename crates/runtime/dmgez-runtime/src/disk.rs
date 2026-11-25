//! Shared logical disk admission and partition selection.

use std::num::NonZeroU32;

use dmgez_udif::reader::{Image, Limits, ReadAt};
use dmgez_vfs::{Disk, Layout, LogicalDisk, PartitionId, Raw, VolumeId};
use thiserror::Error;

/// The representation supplied by a frontend.
#[derive(Debug, Clone, Copy)]
pub enum Encoding {
	/// A qualified UDIF container, possibly signed.
	Udif(Limits),
	/// An explicitly selected raw logical disk.
	Raw,
}

/// Why no volume could be selected.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SelectionError {
	#[error("partition selection requires a recognized partition map")]
	NoPartitionMap,
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
}

/// Admits a held source and calls an operation on its logical disk bytes.
///
/// # Errors
/// Returns container admission errors and errors from the operation.
pub fn with_logical_disk<S: ReadAt, T, E: From<dmgez_udif::Error>>(
	source: S,
	encoding: Encoding,
	read: impl FnOnce(&mut dyn LogicalDisk) -> Result<T, E>,
) -> Result<T, E> {
	match encoding {
		Encoding::Raw => read(&mut Raw(source)),
		Encoding::Udif(limits) => read(&mut Image::open_release(source, limits)?),
	}
}

/// Selects the default admitted volume or a one-based partition from its map.
///
/// # Errors
/// Returns a selection error for missing or ambiguous volumes and invalid slots.
pub fn select_volume(
	disk: &Disk<impl LogicalDisk>,
	slot: Option<NonZeroU32>,
) -> Result<VolumeId, SelectionError> {
	let Some(slot) = slot else {
		return Ok(disk.select_volume(None)?);
	};
	let Layout::Partitioned { scheme, .. } = *disk.layout() else {
		return Err(SelectionError::NoPartitionMap);
	};
	Ok(disk.select_volume(Some(VolumeId::Partition(PartitionId { scheme, slot })))?)
}
