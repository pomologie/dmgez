use std::{collections::BTreeMap, error::Error, rc::Rc};

use dmgez_config::VirtualPath;
use dmgez_finder::{ImageBackgroundIdentity, edit::IconLocationRecord};
use hfsplus_builder::volume::{ForkSource, VolumeDraft, VolumeError};
use hfsplus_core::{ReadAt, names::normalize};

use super::{
	CREATION_DATE, IMAGE_SEPARATOR, Kind, OverlayError, Payload, RebuildError, Snapshot,
	capture::MAX_CAPTURE_BYTES, relative_path,
};

pub(super) type Positions = BTreeMap<String, IconLocationRecord>;
pub(super) type Bindings = BTreeMap<VirtualPath, Binding>;

pub(super) struct Binding {
	pub(super) target: Rc<VirtualPath>,
	pub(super) positions: Positions,
}

pub(super) fn bind<'volume, S: ReadAt>(
	draft: &mut VolumeDraft<'_, Payload<'volume, S>>,
	snapshot: &Snapshot<'volume, S>,
	bindings: &Bindings,
) -> Result<(), RebuildError<S::Error>>
where
	S::Error: Error + 'static,
{
	if bindings.is_empty() {
		return Ok(());
	}
	let label = normalize(&snapshot.name).map_err(VolumeError::Normalization)?;
	let label = String::from_utf16(label.as_units()).map_err(|_| OverlayError::VolumeName)?;
	let mut remaining = MAX_CAPTURE_BYTES;
	for (store, binding) in bindings {
		let missing = || OverlayError::FinderStore {
			path: store.clone(),
		};
		let node = snapshot.entries.get(store).ok_or_else(missing)?;
		let Kind::FinderStore { file, layout } = &node.kind else {
			return Err(missing().into());
		};
		let source = layout.image_background().ok_or_else(missing)?;
		let target = relative_path(&binding.target);
		let parent = target
			.rsplit_once(IMAGE_SEPARATOR)
			.map_or("", |(path, _)| path);
		let final_identity = ImageBackgroundIdentity {
			volume_name: &label,
			volume_created_hfs_seconds: snapshot.dates[CREATION_DATE],
			target_created_hfs_seconds: source.target_created_hfs_seconds,
			parent_catalog_id: catalog_id(draft, parent, store, &binding.target)?,
			target_catalog_id: catalog_id(draft, target, store, &binding.target)?,
			volume_relative_path: binding.target.as_str(),
		};
		let bytes = layout
			.encode_with_image_background(&binding.positions, final_identity)
			.map_err(|source| OverlayError::FinderLayout {
				path: store.clone(),
				source,
			})?;
		let payload = match bytes {
			Some(bytes) => {
				remaining = remaining
					.checked_sub(bytes.capacity())
					.ok_or(OverlayError::Budget)?;
				Payload::bytes(bytes)
			}
			None => file.data.clone(),
		};
		draft.bind_generated_data(relative_path(store), ForkSource::whole(payload))?;
	}
	Ok(())
}

fn catalog_id<S: ReadAt>(
	draft: &VolumeDraft<'_, Payload<'_, S>>,
	path: &str,
	store: &VirtualPath,
	target: &VirtualPath,
) -> Result<u32, RebuildError<S::Error>>
where
	S::Error: Error + 'static,
{
	Ok(draft
		.try_catalog_id(path)?
		.ok_or_else(|| OverlayError::FinderBackgroundTarget {
			store: store.clone(),
			target: target.clone(),
		})?
		.0)
}
