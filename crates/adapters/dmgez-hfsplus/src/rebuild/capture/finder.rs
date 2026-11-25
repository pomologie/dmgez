use std::{collections::BTreeMap, error::Error};

use dmgez_config::VirtualPath;
use dmgez_finder::{edit::EditableStore, inspection::ReadLimits};
use hfsplus_core::{ByteOffset, ReadAt};
use hfsplus_format::volume::CatalogId;
use hfsplus_reader::overflow::{OverflowError, ResolvedFork};

use super::{CREATION_DATE, CaptureError, CapturedNode, Kind, MAX_PATH_BYTES, charge};

pub(super) fn read_store<S: ReadAt>(
	id: CatalogId,
	fork: &ResolvedFork<'_, S>,
	remaining: &mut usize,
) -> Result<EditableStore, CaptureError<S::Error>>
where
	S::Error: Error + 'static,
{
	let limits = ReadLimits::default();
	let len = usize::try_from(fork.len().0)
		.ok()
		.filter(|&len| len <= limits.store_bytes)
		.ok_or(CaptureError::FinderStoreBudget(id))?;
	if len > *remaining {
		return Err(CaptureError::Budget);
	}
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(len)?;
	bytes.resize(len, 0);
	fork.read_exact_at(ByteOffset(0), &mut bytes)
		.map_err(OverflowError::Read)?;
	let layout = EditableStore::open_with_image_background(&bytes, limits)
		.map_err(|error| CaptureError::FinderLayout { id, error })?;
	// Bound cumulative fork reads as well as the layout data retained for edits.
	charge(remaining, len.max(layout.retained_bytes()))?;
	Ok(layout)
}

pub(super) fn backgrounds<S: ReadAt>(
	entries: &BTreeMap<VirtualPath, CapturedNode<'_, S>>,
	volume_name: &str,
	volume_created: u32,
	remaining: &mut usize,
) -> Result<BTreeMap<VirtualPath, VirtualPath>, CaptureError<S::Error>>
where
	S::Error: Error + 'static,
{
	let mut backgrounds = BTreeMap::new();
	for (store_path, store) in entries {
		let Kind::FinderStore { layout, .. } = &store.node.kind else {
			continue;
		};
		let Some(identity) = layout.image_background() else {
			continue;
		};
		if identity.volume_name != volume_name
			|| identity.volume_created_hfs_seconds != volume_created
		{
			return Err(CaptureError::FinderBackgroundVolume(store.id));
		}
		if identity.volume_relative_path.len() > MAX_PATH_BYTES {
			return Err(CaptureError::FinderBackgroundTarget(store.id));
		}
		charge(
			remaining,
			size_of::<(VirtualPath, VirtualPath)>()
				+ store_path.as_str().len()
				+ identity.volume_relative_path.len(),
		)?;
		let target_path = identity
			.volume_relative_path
			.parse()
			.map_err(|_| CaptureError::FinderBackgroundTarget(store.id))?;
		// The walk constructs these keys from the exact stored UTF-16 names,
		// and checks each forward catalog record against its thread.
		let target = entries
			.get(&target_path)
			.filter(|target| matches!(target.node.kind, Kind::File { inode: None, .. }))
			.ok_or(CaptureError::FinderBackgroundTarget(store.id))?;
		if target.id.0 != identity.target_catalog_id
			|| target.parent_id.0 != identity.parent_catalog_id
			|| target.node.metadata.dates[CREATION_DATE] != identity.target_created_hfs_seconds
		{
			return Err(CaptureError::FinderBackgroundIdentity(store.id));
		}
		backgrounds.insert(store_path.clone(), target_path);
	}
	Ok(backgrounds)
}
