use std::{num::NonZeroU32, path::Path};

use dmgez_apfs::{
	ApfsBackend, ApfsContainer, CheckpointDiagnostic, OpenError, TransactionId, VolumeInfo,
	VolumeSelector, WindowSource, probe_partitionless,
};
use dmgez_vfs::{Disk, Filesystem, Limits, LogicalDisk, VolumeId, volume::VolumeView};

use crate::{
	disk::{self, Encoding},
	volume::Error,
};

/// An admitted APFS view borrowing the selected checkpoint and disk.
pub type ApfsView<'volume, 'disk, 'logical> =
	VolumeView<ApfsBackend<'volume, WindowSource<'disk, &'logical mut dyn LogicalDisk>>>;

/// APFS recovery evidence prevents establishing the latest checkpoint.
#[derive(Debug, thiserror::Error)]
#[error(
	"cannot establish the latest APFS checkpoint; inspect checkpoints and explicitly select a retained XID"
)]
pub struct UncertainCheckpoint;

/// Requires an exact checkpoint selection when the latest checkpoint is uncertain.
///
/// # Errors
/// Reports ambiguous chronology when any scan diagnostic remains and the
/// caller has not selected an exact retained transaction.
pub fn require_apfs_checkpoint(
	requested: Option<TransactionId>,
	diagnostics: &[CheckpointDiagnostic],
) -> Result<(), UncertainCheckpoint> {
	// Rejected descriptors can hide a newer checkpoint and its metadata.
	if requested.is_none() && !diagnostics.is_empty() {
		return Err(UncertainCheckpoint);
	}
	Ok(())
}

/// Opens a checkpoint-scoped APFS container in the selected disk window.
///
/// # Errors
/// Returns disk layout, selection or container admission failures.
pub fn with_apfs_disk<T>(
	logical: &mut dyn LogicalDisk,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
	read: impl FnOnce(VolumeId, &ApfsContainer<'_, &mut dyn LogicalDisk>) -> T,
) -> Result<T, Error> {
	let mut disk = Disk::discover(logical, Limits::default())?;
	disk.recognize_partitionless(Filesystem::Apfs, probe_partitionless)?;
	let selected = disk::select_volume(&disk, partition)?;
	let container = ApfsContainer::open(disk.data_window(selected)?, xid)?;
	Ok(read(selected, &container))
}

/// Lends a view of the selected volume and its checked identity.
///
/// # Errors
/// Returns missing or ambiguous selection, unsupported or damaged metadata,
/// budget, or source I/O errors.
pub fn with_apfs_volume<T>(
	selected: VolumeId,
	container: &ApfsContainer<'_, &mut dyn LogicalDisk>,
	selector: Option<VolumeSelector>,
	read: impl FnOnce(&mut ApfsView<'_, '_, '_>, VolumeInfo) -> T,
) -> Result<T, OpenError> {
	let backend = container.open_volume(selector)?;
	let info = backend.info();
	Ok(read(&mut VolumeView::new(selected, backend), info))
}

/// APFS container contents at a selected checkpoint, including recovery evidence.
#[derive(Debug)]
#[non_exhaustive]
pub struct ApfsVolumes {
	pub selected: VolumeId,
	pub checkpoint: TransactionId,
	pub volumes: Vec<VolumeInfo>,
	pub diagnostics: Vec<CheckpointDiagnostic>,
}

/// Lists checked APFS volumes within the selected disk window and checkpoint.
///
/// # Errors
/// Returns disk layout, selection, container admission, or volume-list failures.
pub fn apfs_volumes(
	path: &Path,
	encoding: Encoding,
	partition: Option<NonZeroU32>,
	xid: Option<TransactionId>,
) -> Result<ApfsVolumes, Error> {
	disk::with_path(path, encoding, |logical| {
		with_apfs_disk(logical, partition, xid, |selected, container| {
			Ok(ApfsVolumes {
				selected,
				checkpoint: container.checkpoint(),
				volumes: container.volumes()?,
				diagnostics: container.diagnostics().to_vec(),
			})
		})?
	})
}

/// Opens and qualifies the selected APFS volume under the quarantine policy.
///
/// # Errors
/// Returns source qualification, filesystem admission, or callback errors.
/// An exact checkpoint is required if diagnostic evidence prevents establishing the latest.
#[cfg(all(
	feature = "verified-volume",
	any(target_os = "linux", target_os = "macos")
))]
pub fn with_apfs_path_verified<T, E>(
	path: &Path,
	encoding: Encoding,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	read: impl FnOnce(
		&mut ApfsView<'_, '_, '_>,
		VolumeInfo,
		&dyn Fn() -> std::io::Result<()>,
	) -> Result<T, E>,
) -> Result<T, E>
where
	E: From<std::io::Error> + From<dmgez_udif::Error> + From<Error> + From<OpenError>,
{
	use crate::disk::BorrowedSource;

	super::with_verified_path(path, |source, verify_source| {
		disk::with_logical_disk(BorrowedSource(source), encoding, |logical| {
			with_apfs_disk(logical, partition, xid, |selected, container| {
				require_apfs_checkpoint(xid, container.diagnostics()).map_err(Error::from)?;
				with_apfs_volume(selected, container, selector, |view, info| {
					read(view, info, verify_source)
				})?
			})?
		})
	})
}
