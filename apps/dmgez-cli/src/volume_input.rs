use std::{io, num::NonZeroU32, path::Path};

use dmgez_hfsplus_read::browse::{self, HfsView};
use dmgez_udif::reader::{Image, Limits as ImageLimits};
use dmgez_vfs::{Disk, Layout, Limits as DiskLimits, LogicalDisk, PartitionId, Raw, VolumeId};

use crate::{CliResult, inspect::open_source};

pub(super) fn with_hfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	let source = open_source(image)?;
	let mut raw_disk;
	let mut udif_disk;
	let logical: &mut dyn LogicalDisk = if raw {
		raw_disk = Raw(source);
		&mut raw_disk
	} else {
		udif_disk = Image::open(source, ImageLimits::default())?;
		&mut udif_disk
	};
	let mut disk = Disk::discover(logical, DiskLimits::default())?;
	let selected = select(&disk, partition)?;
	let result = browse::with_view(&mut disk, selected, browse::Limits::default(), read)??;
	disk.data_window(selected)?.read_exact_at(0, &mut [])?;
	Ok(result)
}

fn select(disk: &Disk<impl LogicalDisk>, slot: Option<NonZeroU32>) -> CliResult<VolumeId> {
	let Some(slot) = slot else {
		return Ok(disk.select_volume(None)?);
	};
	let Layout::Partitioned { scheme, .. } = *disk.layout() else {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"partition selection requires a recognized partition map",
		)
		.into());
	};
	Ok(disk.select_volume(Some(VolumeId::Partition(PartitionId { scheme, slot })))?)
}
