use std::{io, num::NonZeroU32, path::Path};

#[cfg(feature = "apfs-read")]
use dmgez_apfs_read::{
	ApfsBackend, ApfsContainer, CheckpointDiagnostic, TransactionId, VolumeInfo, VolumeSelector,
	WindowSource, probe_partitionless,
};
#[cfg(feature = "hfsplus-read")]
use dmgez_hfsplus_read::browse::{self, HfsView, VolumeSignature};
use dmgez_udif::reader::{FileSource, Image, Limits as ImageLimits, ReadAt};
use dmgez_vfs::{Disk, Layout, Limits as DiskLimits, LogicalDisk, PartitionId, Raw, VolumeId};
#[cfg(feature = "apfs-read")]
use dmgez_vfs::{Filesystem, volume::VolumeView};

use crate::{CliResult, inspect::open_source};

pub(super) struct BorrowedSource<'source>(pub(super) &'source FileSource);

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

#[cfg(feature = "hfsplus-read")]
pub(super) fn with_hfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	with_hfs_source(image, raw, partition, |view, _| read(view))
}

#[cfg(all(feature = "diff", feature = "hfsplus-read"))]
pub(super) fn with_hfs_view_with_signature<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>, VolumeSignature) -> CliResult<T>,
) -> CliResult<T> {
	with_hfs_source(image, raw, partition, read)
}

#[cfg(feature = "hfsplus-read")]
fn with_hfs_source<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>, VolumeSignature) -> CliResult<T>,
) -> CliResult<T> {
	let source = open_source(image)?;
	let result = with_source(&source, raw, partition, read)?;
	source.check_unchanged()?;
	Ok(result)
}

#[cfg(all(
	any(feature = "hfsplus-extract", feature = "hfsplus-install"),
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
	with_verified_source(image, |source, verify_source| {
		with_source(source, raw, partition, |view, _| read(view, verify_source))
	})
}

#[cfg(all(
	feature = "project-release",
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
))]
pub(super) fn with_hfs_file_verified<T>(
	file: &std::fs::File,
	read: impl FnOnce(
		&mut HfsView<'_, '_, &mut dyn LogicalDisk>,
		&dyn Fn() -> io::Result<()>,
	) -> CliResult<T>,
) -> CliResult<T> {
	with_verified_file(file, |source, verify_source| {
		with_source(source, false, None, |view, _| read(view, verify_source))
	})
}

#[cfg(feature = "hfsplus-read")]
fn with_source<T>(
	source: &FileSource,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>, VolumeSignature) -> CliResult<T>,
) -> CliResult<T> {
	with_logical_disk(source, raw, |logical| {
		with_hfs_disk(logical, partition, read)
	})
}

#[cfg(feature = "hfsplus-read")]
pub(super) fn with_hfs_disk<T>(
	logical: &mut dyn LogicalDisk,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>, VolumeSignature) -> CliResult<T>,
) -> CliResult<T> {
	let mut disk = Disk::discover(logical, DiskLimits::default())?;
	let selected = select(&disk, partition)?;
	browse::with_view_signature(&mut disk, selected, browse::Limits::default(), read)?
}

#[cfg(feature = "apfs-read")]
type ApfsView<'volume, 'disk, 'logical> =
	VolumeView<ApfsBackend<'volume, WindowSource<'disk, &'logical mut dyn LogicalDisk>>>;

#[cfg(feature = "apfs-read")]
pub(super) fn with_apfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	read: impl FnOnce(&mut ApfsView<'_, '_, '_>, &VolumeInfo) -> CliResult<T>,
) -> CliResult<T> {
	with_apfs_container(image, raw, partition, xid, |selected, container| {
		use io::Write as _;

		let backend = container.open_volume(selector)?;
		let info = backend.info();
		let mut stderr = io::stderr().lock();
		for diagnostic in container.diagnostics() {
			writeln!(stderr, "APFS checkpoint warning: {diagnostic:?}")?;
		}
		drop(stderr);
		read(&mut VolumeView::new(selected, backend), &info)
	})
}

#[cfg(all(feature = "diff", feature = "apfs-read"))]
pub(super) fn with_apfs_view_for_comparison<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	read: impl FnOnce(&mut ApfsView<'_, '_, '_>, &VolumeInfo) -> CliResult<T>,
) -> CliResult<T> {
	with_apfs_container(image, raw, partition, xid, |selected, container| {
		require_apfs_checkpoint(xid, container.diagnostics())?;
		let backend = container.open_volume(selector)?;
		let info = backend.info();
		read(&mut VolumeView::new(selected, backend), &info)
	})
}

#[cfg(all(
	any(feature = "apfs-extract", feature = "apfs-install"),
	any(target_os = "linux", target_os = "macos")
))]
pub(super) fn with_apfs_view_verified<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	read: impl FnOnce(
		&mut ApfsView<'_, '_, '_>,
		&VolumeInfo,
		&dyn Fn() -> io::Result<()>,
	) -> CliResult<T>,
) -> CliResult<T> {
	with_verified_source(image, |source, verify_source| {
		with_apfs_container_source(source, raw, partition, xid, |selected, container| {
			require_apfs_checkpoint(xid, container.diagnostics())?;
			let backend = container.open_volume(selector)?;
			let info = backend.info();
			read(
				&mut VolumeView::new(selected, backend),
				&info,
				verify_source,
			)
		})
	})
}

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
pub(super) fn require_apfs_checkpoint(
	requested: Option<TransactionId>,
	diagnostics: &[CheckpointDiagnostic],
) -> io::Result<()> {
	// Rejected descriptors can hide a newer checkpoint and its metadata.
	if requested.is_none() && !diagnostics.is_empty() {
		return Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"cannot establish the latest APFS checkpoint; inspect checkpoints and explicitly select a retained XID",
		));
	}
	Ok(())
}

#[cfg(feature = "apfs-read")]
pub(super) struct ApfsVolumes {
	pub(super) selected: VolumeId,
	pub(super) checkpoint: TransactionId,
	pub(super) volumes: Vec<VolumeInfo>,
	pub(super) diagnostics: Vec<CheckpointDiagnostic>,
}

#[cfg(feature = "apfs-read")]
pub(super) fn apfs_volumes(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
) -> CliResult<ApfsVolumes> {
	with_apfs_container(image, raw, partition, xid, |selected, container| {
		Ok(ApfsVolumes {
			selected,
			checkpoint: container.checkpoint(),
			volumes: container.volumes()?,
			diagnostics: container.diagnostics().to_vec(),
		})
	})
}

#[cfg(feature = "apfs-read")]
fn with_apfs_container<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
	read: impl FnOnce(VolumeId, &ApfsContainer<'_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	let source = open_source(image)?;
	let result = with_apfs_container_source(&source, raw, partition, xid, read)?;
	source.check_unchanged()?;
	Ok(result)
}

#[cfg(feature = "apfs-read")]
fn with_apfs_container_source<T>(
	source: &FileSource,
	raw: bool,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
	read: impl FnOnce(VolumeId, &ApfsContainer<'_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	with_logical_disk(source, raw, |logical| {
		with_apfs_disk(logical, partition, xid, read)
	})
}

#[cfg(feature = "apfs-read")]
pub(super) fn with_apfs_disk<T>(
	logical: &mut dyn LogicalDisk,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
	read: impl FnOnce(VolumeId, &ApfsContainer<'_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	let mut disk = Disk::discover(logical, DiskLimits::default())?;
	disk.recognize_partitionless(Filesystem::Apfs, probe_partitionless)?;
	let selected = select(&disk, partition)?;
	let container = ApfsContainer::open(disk.data_window(selected)?, xid)?;
	read(selected, &container)
}

#[cfg(all(
	any(
		feature = "hfsplus-extract",
		feature = "apfs-extract",
		feature = "hfsplus-install",
		feature = "apfs-install"
	),
	any(target_os = "linux", target_os = "macos")
))]
fn with_verified_source<T>(
	image: &Path,
	read: impl FnOnce(&FileSource, &dyn Fn() -> io::Result<()>) -> CliResult<T>,
) -> CliResult<T> {
	let file = std::fs::File::open(image)?;
	with_verified_file(&file, read)
}

#[cfg(all(
	any(
		feature = "hfsplus-extract",
		feature = "apfs-extract",
		feature = "hfsplus-install",
		feature = "apfs-install"
	),
	any(target_os = "linux", target_os = "macos")
))]
fn with_verified_file<T>(
	file: &std::fs::File,
	read: impl FnOnce(&FileSource, &dyn Fn() -> io::Result<()>) -> CliResult<T>,
) -> CliResult<T> {
	let source = FileSource::new(file.try_clone()?)?;
	let verify_source = || {
		dmgez_operations::extract::host::check_source_quarantine(file)?;
		source.check_unchanged().map_err(io::Error::other)
	};
	verify_source()?;
	read(&source, &verify_source)
}

fn with_logical_disk<T>(
	source: &FileSource,
	raw: bool,
	read: impl FnOnce(&mut dyn LogicalDisk) -> CliResult<T>,
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
	read(logical)
}

pub(super) fn select(
	disk: &Disk<impl LogicalDisk>,
	slot: Option<NonZeroU32>,
) -> CliResult<VolumeId> {
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
