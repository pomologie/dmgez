use std::error::Error as StdError;

use apfs_container::{Container, Error as ContainerError, LookupOptions, OpenOptions, Volume};
use apfs_core::ReadAt;
use apfs_reader::{Error as ReaderError, TreeLimits, VolumeView};
use dmgez_vfs::{LogicalDisk, Window};
use thiserror::Error;

use crate::{
	ApfsBackend, CheckpointDiagnostic, ObjectId, SourceError, TransactionId, VolumeName,
	VolumeRole, WindowSource,
};

/// Identifies a volume within one APFS container checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeSelector {
	ObjectId(ObjectId),
	Uuid([u8; 16]),
}

/// Checked metadata for a volume as it appears in the selected checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VolumeInfo {
	pub object_id: ObjectId,
	pub uuid: [u8; 16],
	pub name: VolumeName,
	pub role: VolumeRole,
	pub checkpoint: TransactionId,
}

impl From<&Volume> for VolumeInfo {
	fn from(volume: &Volume) -> Self {
		Self {
			object_id: volume.mapping.id,
			uuid: volume.superblock.uuid,
			name: volume.superblock.name.clone(),
			role: volume.superblock.role,
			checkpoint: volume.checkpoint,
		}
	}
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError<E: StdError + 'static = SourceError> {
	#[error(transparent)]
	Container(#[from] ContainerError<E>),
	#[error(transparent)]
	Reader(#[from] ReaderError<E>),
	#[error("APFS checkpoint has no volumes")]
	NoVolumes,
	#[error("APFS checkpoint has {0} volumes; select one by UUID or object ID")]
	AmbiguousVolumes(usize),
	#[error("APFS volume selector {0:?} was not found in the chosen checkpoint")]
	UnknownVolume(VolumeSelector),
	#[error("could not allocate the checked APFS volume list")]
	Allocation,
}

/// An APFS container bound to one checkpoint and its original disk window.
pub struct ApfsContainer<'disk, S> {
	container: Container<WindowSource<'disk, S>>,
}

impl<'disk, S: LogicalDisk> ApfsContainer<'disk, S> {
	/// Opens an exact retained checkpoint or the highest complete checkpoint.
	///
	/// # Errors
	/// Fails on an absent requested XID, unsupported container profile, damaged
	/// metadata, bounds or budget violations, or a failed source read.
	pub fn open(window: Window<'disk, S>, xid: Option<TransactionId>) -> Result<Self, OpenError> {
		Ok(Self {
			container: open_container(WindowSource::new(window), xid)?,
		})
	}

	/// Transaction ID used for all volume lookups on this container.
	#[must_use]
	pub fn checkpoint(&self) -> TransactionId {
		self.container.superblock().object.transaction
	}

	/// Checkpoint scan diagnostics, including evidence of fallback.
	#[must_use]
	pub fn diagnostics(&self) -> &[CheckpointDiagnostic] {
		self.container.diagnostics()
	}

	/// Checked volume metadata in original container slot order.
	///
	/// # Errors
	/// Fails if any listed volume cannot be admitted, if the table is
	/// inconsistent, or if an allocation, lookup budget or source read fails.
	pub fn volumes(&self) -> Result<Vec<VolumeInfo>, OpenError> {
		let volumes = self.container.volumes(LookupOptions::default())?;
		let mut info = Vec::new();
		info.try_reserve_exact(volumes.len())
			.map_err(|_| OpenError::Allocation)?;
		info.extend(volumes.iter().map(VolumeInfo::from));
		Ok(info)
	}

	/// Opens the specified volume, or the only volume in the container.
	/// An explicit object ID resolves independently. UUID and implicit
	/// selection require a checked volume list for the entire checkpoint.
	///
	/// # Errors
	/// Fails on unknown selectors, empty or ambiguous implicit selection,
	/// unsupported or damaged filesystem metadata, and budget or I/O failures.
	pub fn open_volume(
		&self,
		requested: Option<VolumeSelector>,
	) -> Result<ApfsBackend<'_, WindowSource<'disk, S>>, OpenError> {
		Ok(ApfsBackend::new(open_volume(&self.container, requested)?))
	}
}

pub(super) fn open_container<S: ReadAt>(
	source: S,
	xid: Option<TransactionId>,
) -> Result<Container<S>, ContainerError<S::Error>> {
	let options = OpenOptions::default();
	match xid {
		Some(xid) => Container::open_at(source, options, xid),
		None => Container::open(source, options),
	}
}

pub(super) fn open_volume<S: ReadAt>(
	container: &Container<S>,
	requested: Option<VolumeSelector>,
) -> Result<VolumeView<'_, S>, OpenError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let options = LookupOptions::default();
	let id = match requested {
		Some(VolumeSelector::ObjectId(id)) => id,
		Some(selector @ VolumeSelector::Uuid(uuid)) => {
			container
				.volumes(options)?
				.into_iter()
				.find(|volume| volume.superblock.uuid == uuid)
				.ok_or(OpenError::UnknownVolume(selector))?
				.mapping
				.id
		}
		None => match container.volumes(options)?.as_slice() {
			[only] => only.mapping.id,
			[] => return Err(OpenError::NoVolumes),
			volumes => return Err(OpenError::AmbiguousVolumes(volumes.len())),
		},
	};
	VolumeView::open(container, id, options, TreeLimits::default())?
		.ok_or(OpenError::UnknownVolume(VolumeSelector::ObjectId(id)))
}
