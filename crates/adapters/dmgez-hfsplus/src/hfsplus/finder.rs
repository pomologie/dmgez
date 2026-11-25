use std::{borrow::Cow, error::Error as StdError, mem::size_of};

use dmgez_builder::{
	planning::PreparedBuild,
	presentation::{CatalogNames, GeneratedAsset, ImageBackgroundIdentity},
};
use dmgez_config::VirtualPath;
use dmgez_udif::writer::Cancellation;
use hfsplus_builder::volume::{
	EntryKind, EntryMetadata, ForkSource, ManifestEntry, VolumeDraft, VolumeError,
};
use hfsplus_core::names::normalize;

use super::{
	Error, IMAGE_PATH_SEPARATOR, Payload, charge, charge_many, check_cancelled,
	metadata::{DEFAULT_FILE_MODE, DEFAULT_FOLDER_MODE, generated_metadata},
	relative,
};

// TN1150 FileInfo and FolderInfo place the Finder flags at the same offset.
const FINDER_FLAGS_OFFSET: usize = 8;
const FINDER_FLAGS_END: usize = FINDER_FLAGS_OFFSET + size_of::<u16>();
const FINDER_INVISIBLE: u16 = 0x4000;

pub(super) const CATALOG_NAMES: CatalogNames = CatalogNames::new(stored_name);

fn stored_name(name: &str) -> Result<String, Box<dyn StdError + Send + Sync>> {
	Ok(String::from_utf16(normalize(name)?.as_units())?)
}

pub(super) fn account_assets(
	prepared: &PreparedBuild,
	remaining: &mut usize,
) -> Result<usize, Error> {
	let ready = prepared.finder_assets();
	let directories = prepared.finder_directories();
	let deferred = prepared.deferred_finder_stores();
	charge_many::<GeneratedAsset>(remaining, ready.len())?;
	for asset in ready {
		charge(remaining, asset.bytes.capacity())?;
		charge(remaining, asset.path.as_str().len())?;
	}
	charge_many::<VirtualPath>(remaining, directories.len())?;
	for directory in directories {
		charge(remaining, directory.as_str().len())?;
	}
	charge(remaining, prepared.deferred_finder_metadata_bytes())?;
	// Reserve the maximum encoded capacity before granting the HFS+ draft its
	// independent metadata allowance. Binding must fit within this reservation.
	for store in deferred {
		charge(remaining, store.max_encoded_bytes())?;
	}
	ready
		.len()
		.checked_add(directories.len())
		.and_then(|count| count.checked_add(deferred.len()))
		.ok_or(Error::MetadataBudget)
}

pub(super) fn reserve_assets<'build>(
	prepared: &'build PreparedBuild,
	entries: &mut Vec<ManifestEntry<'build, Payload<'build>>>,
	date: u32,
	cancellation: &Cancellation,
) -> Result<(), Error> {
	let directories = prepared.finder_directories();
	for directory in directories {
		check_cancelled(cancellation)?;
		entries.push(ManifestEntry {
			path: relative(directory)?,
			metadata: invisible_metadata(DEFAULT_FOLDER_MODE, date),
			kind: EntryKind::Folder,
		});
	}
	for asset in prepared.finder_assets() {
		check_cancelled(cancellation)?;
		let metadata = if asset
			.path
			.as_str()
			.rsplit_once(IMAGE_PATH_SEPARATOR)
			.is_some_and(|(parent, _)| directories.iter().any(|dir| dir.as_str() == parent))
		{
			invisible_metadata(DEFAULT_FILE_MODE, date)
		} else {
			generated_metadata(DEFAULT_FILE_MODE, date)
		};
		entries.push(ManifestEntry {
			path: relative(&asset.path)?,
			metadata,
			kind: EntryKind::GeneratedFile { resource: None },
		});
	}
	for store in prepared.deferred_finder_stores() {
		check_cancelled(cancellation)?;
		entries.push(ManifestEntry {
			path: relative(store.path())?,
			metadata: generated_metadata(DEFAULT_FILE_MODE, date),
			kind: EntryKind::GeneratedFile { resource: None },
		});
	}
	Ok(())
}

pub(super) fn bind_assets<'build>(
	prepared: &'build PreparedBuild,
	draft: &mut VolumeDraft<'_, Payload<'build>>,
	volume_name: &str,
	date: u32,
	cancellation: &Cancellation,
) -> Result<(), Error> {
	for asset in prepared.finder_assets() {
		check_cancelled(cancellation)?;
		draft.bind_generated_data(
			relative(&asset.path)?,
			ForkSource::whole(Payload::Bytes(Cow::Borrowed(&asset.bytes))),
		)?;
	}
	let deferred = prepared.deferred_finder_stores();
	if deferred.is_empty() {
		return Ok(());
	}
	// The HFS+ draft has admitted and decomposed the label via the same codec.
	let normalized = normalize(volume_name).map_err(VolumeError::Normalization)?;
	let volume_name =
		String::from_utf16(normalized.as_units()).map_err(Error::BackgroundVolumeName)?;
	for store in deferred {
		check_cancelled(cancellation)?;
		let absolute_target = store.background_path();
		let target = relative(absolute_target)?;
		let parent = target
			.rsplit_once(IMAGE_PATH_SEPARATOR)
			.map_or("", |(parent, _)| parent);
		let identity = ImageBackgroundIdentity {
			volume_name: &volume_name,
			volume_created_hfs_seconds: date,
			target_created_hfs_seconds: date,
			parent_catalog_id: catalog_id(draft, parent)?,
			target_catalog_id: catalog_id(draft, target)?,
			volume_relative_path: absolute_target.as_str(),
		};
		let bytes = store.render(identity)?;
		check_cancelled(cancellation)?;
		draft.bind_generated_data(
			relative(store.path())?,
			ForkSource::whole(Payload::Bytes(Cow::Owned(bytes))),
		)?;
	}
	Ok(())
}

fn catalog_id(draft: &VolumeDraft<'_, Payload<'_>>, path: &str) -> Result<u32, Error> {
	draft
		.try_catalog_id(path)?
		.map(|id| id.0)
		.ok_or_else(|| Error::BackgroundIdentity { path: path.into() })
}

fn invisible_metadata(mode: u16, date: u32) -> EntryMetadata {
	let mut metadata = generated_metadata(mode, date);
	metadata.finder_info[FINDER_FLAGS_OFFSET..FINDER_FLAGS_END]
		.copy_from_slice(&FINDER_INVISIBLE.to_be_bytes());
	metadata
}
