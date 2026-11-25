//! Bounded access to selected HFS+ and APFS volumes on an admitted disk.

use dmgez_udif::Error as ImageError;
use thiserror::Error;

use crate::disk::SelectionError;

#[cfg(feature = "apfs-volume")]
mod apfs;
#[cfg(all(
	feature = "apfs-volume",
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub use apfs::with_apfs_path_verified;
#[cfg(feature = "apfs-volume")]
pub use apfs::{
	ApfsView, ApfsVolumes, UncertainCheckpoint, apfs_volumes, require_apfs_checkpoint,
	with_apfs_disk, with_apfs_volume, with_apfs_window,
};
#[cfg(feature = "hfsplus-volume")]
mod hfsplus;
#[cfg(feature = "hfsplus-volume")]
pub use hfsplus::with_hfs_disk;
#[cfg(all(
	feature = "hfsplus-volume",
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub use hfsplus::{with_hfs_file_verified, with_hfs_path_verified};
#[cfg(all(
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
mod verified;
#[cfg(all(
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub use verified::{with_verified_file, with_verified_path};

/// Failure admitting an image, disk window or filesystem volume.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Image(#[from] ImageError),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[error(transparent)]
	Selection(#[from] SelectionError),
	#[cfg(feature = "hfsplus-volume")]
	#[error(transparent)]
	Hfsplus(#[from] dmgez_hfsplus::browse::Error),
	#[cfg(feature = "apfs-volume")]
	#[error(transparent)]
	Apfs(#[from] dmgez_apfs::OpenError),
	#[cfg(feature = "apfs-volume")]
	#[error(transparent)]
	ApfsProbe(#[from] dmgez_apfs::ProbeError),
	#[cfg(feature = "apfs-volume")]
	#[error(transparent)]
	Checkpoint(#[from] UncertainCheckpoint),
}
