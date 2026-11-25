use std::{io, num::NonZeroU32, path::Path};

use dmgez_hfsplus_read::browse::{self, HfsView};
use dmgez_udif::reader::{FileSource, Image, Limits as ImageLimits, ReadAt};
use dmgez_vfs::{Disk, Layout, Limits as DiskLimits, LogicalDisk, PartitionId, Raw, VolumeId};

use crate::{CliResult, inspect::open_source};

struct BorrowedSource<'source>(&'source FileSource);

impl ReadAt for BorrowedSource<'_> {
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

pub(super) fn with_hfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	let source = open_source(image)?;
	let result = with_source(&source, raw, partition, read)?;
	source.check_unchanged()?;
	Ok(result)
}

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
pub(super) fn with_hfs_view_verified<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(
		&mut HfsView<'_, '_, &mut dyn LogicalDisk>,
		&dyn Fn() -> io::Result<()>,
	) -> CliResult<T>,
) -> CliResult<T> {
	let file = std::fs::File::open(image)?;
	let source = FileSource::new(file.try_clone()?)?;
	let verify_source = || {
		dmgez_operations::extract::host::check_source_quarantine(&file)?;
		source.check_unchanged().map_err(io::Error::other)
	};
	verify_source()?;
	with_source(&source, raw, partition, |view| read(view, &verify_source))
}

fn with_source<T>(
	source: &FileSource,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	let borrowed = BorrowedSource(source);
	let mut raw_disk;
	let mut udif_disk;
	let logical: &mut dyn LogicalDisk = if raw {
		raw_disk = Raw(borrowed);
		&mut raw_disk
	} else {
		udif_disk = Image::open(borrowed, ImageLimits::default())?;
		&mut udif_disk
	};
	let mut disk = Disk::discover(logical, DiskLimits::default())?;
	let selected = select(&disk, partition)?;
	browse::with_view(&mut disk, selected, browse::Limits::default(), read)?
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
