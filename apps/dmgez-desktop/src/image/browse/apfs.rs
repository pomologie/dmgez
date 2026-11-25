use dmgez_apfs::{
	AdapterError, CheckpointDiagnostic, ObjectId, SourceError, TransactionId, VolumeInfo,
	VolumeSelector,
};
use dmgez_runtime::volume::{ApfsView, with_apfs_volume, with_apfs_window};
use dmgez_vfs::{LogicalDisk, VolumeId, volume::RawName};

use super::{Error, Filesystem, Location, Observation, Request, query, search};

pub(super) type BackendError = AdapterError<SourceError>;

/// A selected filesystem identity at one retained container checkpoint.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Selection {
	pub object_id: ObjectId,
	pub checkpoint: TransactionId,
	uuid: [u8; 16],
}

impl From<&VolumeInfo> for Selection {
	fn from(info: &VolumeInfo) -> Self {
		Self {
			object_id: info.object_id,
			checkpoint: info.checkpoint,
			uuid: info.uuid,
		}
	}
}

#[derive(Debug)]
pub(crate) struct Catalog {
	pub window: VolumeId,
	pub checkpoint: TransactionId,
	pub volumes: Vec<VolumeInfo>,
	pub diagnostics: Vec<CheckpointDiagnostic>,
}

impl Catalog {
	pub(crate) fn root(&self, info: &VolumeInfo) -> Location {
		Location {
			apfs: Some(info.into()),
			..Location::root(self.window)
		}
	}
}

pub(super) fn discover(
	logical: &mut dyn LogicalDisk,
	window: VolumeId,
) -> Result<Observation, Error> {
	with_apfs_window(logical, window, None, |window, container| {
		Ok(Observation::ApfsCatalog(Catalog {
			window,
			checkpoint: container.checkpoint(),
			volumes: container.volumes()?,
			diagnostics: container.diagnostics().to_vec(),
		}))
	})?
}

pub(super) fn browse(
	logical: &mut dyn LogicalDisk,
	window: VolumeId,
	selection: Selection,
	request: Request,
	context: query::Context<'_>,
) -> Result<Observation, Error> {
	let selector = Some(VolumeSelector::ObjectId(selection.object_id));
	with_apfs_window(
		logical,
		window,
		Some(selection.checkpoint),
		|selected, container| {
			with_apfs_volume(selected, container, selector, |view, info| {
				if selection != Selection::from(&info) {
					return Err(Error::ApfsVolumeChanged);
				}
				query::query_volume(
					view,
					Filesystem::Apfs,
					request,
					search,
					context,
					container.logical_window(),
				)
			})?
		},
	)?
}

fn search(
	view: &mut ApfsView<'_, '_, '_>,
	location: Location,
	query: String,
) -> Result<Observation, Error> {
	search::folder_search(view, location, query, |name, query| match name {
		RawName::Utf8(name) => Ok(name.contains(query)),
		_ => Err(BackendError::NameEncoding),
	})
}

#[cfg(test)]
mod tests;
