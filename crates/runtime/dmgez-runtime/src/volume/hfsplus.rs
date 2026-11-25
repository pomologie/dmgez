use std::num::NonZeroU32;

use dmgez_hfsplus::browse::{self, HfsView, VolumeSignature};
use dmgez_vfs::{Disk, Limits, LogicalDisk};

use crate::{disk::select_volume, volume::Error};

/// Opens the selected HFS+ or HFSX extent and lends its scoped view.
///
/// # Errors
/// Returns layout, selection, or filesystem admission failures.
pub fn with_hfs_disk<T>(
	logical: &mut dyn LogicalDisk,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>, VolumeSignature) -> T,
) -> Result<T, Error> {
	let mut disk = Disk::discover(logical, Limits::default())?;
	let selected = select_volume(&disk, partition)?;
	Ok(browse::with_view_signature(
		&mut disk,
		selected,
		browse::Limits::default(),
		read,
	)?)
}

/// Opens and qualifies the HFS+ view of a source path under the quarantine policy.
///
/// # Errors
/// Returns source qualification, filesystem admission, or callback errors.
#[cfg(all(
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub fn with_hfs_path_verified<
	T,
	E: From<std::io::Error> + From<dmgez_udif::Error> + From<Error>,
>(
	path: &std::path::Path,
	encoding: crate::disk::Encoding,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(
		&mut HfsView<'_, '_, &mut dyn LogicalDisk>,
		&dyn Fn() -> std::io::Result<()>,
	) -> Result<T, E>,
) -> Result<T, E> {
	use crate::disk::{self, BorrowedSource};

	super::with_verified_path(path, |source, verify_source| {
		disk::with_logical_disk(BorrowedSource(source), encoding, |logical| {
			with_hfs_disk(logical, partition, |view, _| read(view, verify_source))
		})?
	})
}

/// Admits the HFS+ view of a held UDIF file under the quarantine policy.
///
/// # Errors
/// Returns source qualification, filesystem admission, or callback errors.
#[cfg(all(
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub fn with_hfs_file_verified<
	T,
	E: From<std::io::Error> + From<dmgez_udif::Error> + From<Error>,
>(
	file: &std::fs::File,
	read: impl FnOnce(
		&mut HfsView<'_, '_, &mut dyn LogicalDisk>,
		&dyn Fn() -> std::io::Result<()>,
	) -> Result<T, E>,
) -> Result<T, E> {
	use crate::disk::{self, BorrowedSource, Encoding};

	super::with_verified_file(file, |source, verify_source| {
		disk::with_logical_disk(
			BorrowedSource(source),
			Encoding::Udif(Default::default()),
			|logical| with_hfs_disk(logical, None, |view, _| read(view, verify_source)),
		)?
	})
}
