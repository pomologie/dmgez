//! Shared logical disk admission and partition selection.

use std::{num::NonZeroU32, path::Path};

use dmgez_udif::reader::{FileSource, Image, Limits, ReadAt};
use dmgez_vfs::{Disk, Layout, LogicalDisk, PartitionId, Raw, VolumeId};
use thiserror::Error;

use crate::image::open_file;

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

/// Borrows a captured source for use with an owning logical disk reader.
#[derive(Debug)]
pub struct BorrowedSource<'source, S = FileSource>(pub &'source S);

impl<S: ReadAt> ReadAt for BorrowedSource<'_, S> {
	fn len(&self) -> u64 {
		self.0.len()
	}

	fn check_unchanged(&self) -> Result<(), dmgez_udif::Error> {
		self.0.check_unchanged()
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), dmgez_udif::Error> {
		self.0.read_exact_at(offset, output)
	}
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

/// Follows one file path, reads the logical disk, and rechecks the held source.
///
/// # Errors
/// Returns admission, callback or source change errors.
pub fn with_path<T, E: From<dmgez_udif::Error>>(
	path: &Path,
	encoding: Encoding,
	read: impl FnOnce(&mut dyn LogicalDisk) -> Result<T, E>,
) -> Result<T, E> {
	let source = open_file(path)?;
	let result = with_logical_disk(BorrowedSource(&source), encoding, read)?;
	source.check_unchanged()?;
	Ok(result)
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
