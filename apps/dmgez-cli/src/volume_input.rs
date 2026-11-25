use std::{num::NonZeroU32, path::Path};

#[cfg(any(
	feature = "apfs-read",
	all(
		any(feature = "hfsplus-extract", feature = "hfsplus-install"),
		any(target_os = "linux", target_os = "macos")
	)
))]
use std::io;

#[cfg(feature = "apfs-read")]
use dmgez_apfs_read::{TransactionId, VolumeInfo, VolumeSelector};
#[cfg(feature = "hfsplus-read")]
use dmgez_hfsplus::browse::HfsView;
use dmgez_runtime::{
	disk::{self, Encoding},
	volume,
};
#[cfg(feature = "hfsplus-read")]
use dmgez_vfs::LogicalDisk;

use crate::CliResult;

#[cfg(feature = "hfsplus-read")]
pub(super) fn with_hfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	read: impl FnOnce(&mut HfsView<'_, '_, &mut dyn LogicalDisk>) -> CliResult<T>,
) -> CliResult<T> {
	disk::with_path(image, encoding(raw), |logical| {
		volume::with_hfs_disk(logical, partition, |view, _| read(view))?
	})
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
	volume::with_hfs_path_verified(image, encoding(raw), partition, read)
}

#[cfg(feature = "apfs-read")]
pub(super) fn with_apfs_view<T>(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	read: impl FnOnce(&mut volume::ApfsView<'_, '_, '_>, &VolumeInfo) -> CliResult<T>,
) -> CliResult<T> {
	disk::with_path(image, encoding(raw), |logical| {
		volume::with_apfs_disk(logical, partition, xid, |selected, container| {
			volume::with_apfs_volume(selected, container, selector, |view, info| {
				use io::Write as _;

				let mut stderr = io::stderr().lock();
				for diagnostic in container.diagnostics() {
					writeln!(stderr, "APFS checkpoint warning: {diagnostic:?}")?;
				}
				drop(stderr);
				read(view, &info)
			})?
		})?
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
		&mut volume::ApfsView<'_, '_, '_>,
		&VolumeInfo,
		&dyn Fn() -> io::Result<()>,
	) -> CliResult<T>,
) -> CliResult<T> {
	volume::with_apfs_path_verified(
		image,
		encoding(raw),
		partition,
		selector,
		xid,
		|view, info, verify_source| read(view, &info, verify_source),
	)
}

#[cfg(feature = "apfs-read")]
pub(super) fn apfs_volumes(
	image: &Path,
	raw: bool,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
) -> CliResult<volume::ApfsVolumes> {
	Ok(volume::apfs_volumes(image, encoding(raw), partition, xid)?)
}

fn encoding(raw: bool) -> Encoding {
	if raw {
		Encoding::Raw
	} else {
		Encoding::Udif(Default::default())
	}
}
