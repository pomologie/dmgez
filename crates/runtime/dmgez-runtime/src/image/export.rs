use std::{num::NonZeroU32, path::Path};

use dmgez_core::output::{PublishError, publish_new};
use dmgez_udif::reader::{Image, Limits};
use dmgez_vfs::{Disk, Layout, Limits as DiskLimits, PartitionId, VolumeId};
use thiserror::Error;

use super::open_file;

const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Why an admitted image or partition could not be exported.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExportError {
	#[error(transparent)]
	Image(#[from] dmgez_udif::Error),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[error("partition export requires a recognized partition map")]
	NoPartitionMap,
	#[error(transparent)]
	ImagePublication(#[from] PublishError<dmgez_udif::Error>),
	#[error(transparent)]
	PartitionPublication(#[from] PublishError<dmgez_vfs::Error>),
}

/// Exports the entire logical disk or one selected partition to a fresh path.
///
/// # Errors
/// Returns admission, copy or publication failures. Existing output is preserved.
pub fn export_raw(
	path: &Path,
	destination: &Path,
	partition: Option<NonZeroU32>,
) -> Result<(), ExportError> {
	let mut image = Image::open_release(open_file(path)?, Limits::default())?;
	if let Some(slot) = partition {
		let mut disk = Disk::discover(&mut image, DiskLimits::default())?;
		let Layout::Partitioned { scheme, .. } = *disk.layout() else {
			return Err(ExportError::NoPartitionMap);
		};
		let mut partition = disk.window(VolumeId::Partition(PartitionId { scheme, slot }))?;
		let mut scratch = vec![0; COPY_BUFFER_BYTES];
		publish_new(
			destination,
			|stage| partition.write_raw_to(stage, &mut scratch, || false),
			|_, _| Ok(()),
		)?;
	} else {
		publish_new(
			destination,
			|stage| image.write_raw_to(stage),
			|_, _| Ok(()),
		)?;
	}
	Ok(())
}
