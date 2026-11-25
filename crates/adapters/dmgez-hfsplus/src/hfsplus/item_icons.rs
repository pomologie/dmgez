use std::borrow::Cow;

use dmgez_builder::{
	manifest::Entry, planning::PreparedBuild, preflight::FilesystemProfile, presentation::item_icon,
};
use hfsplus_builder::volume::{EntryKind, ForkSource, ManifestEntry};

use super::{
	Error, Payload, charge,
	metadata::{DEFAULT_FILE_MODE, generated_metadata},
	relative,
};

pub(super) fn account_assets(
	prepared: &PreparedBuild,
	remaining: &mut usize,
) -> Result<usize, Error> {
	charge(remaining, prepared.item_icon_metadata_bytes())?;
	Ok(prepared
		.manifest()
		.entries()
		.iter()
		.filter(|entry| prepared.item_icon_child(&entry.destination).is_some())
		.count())
}

pub(super) fn reject_shared(entry: &Entry) -> Result<(), Error> {
	if entry.finder.custom_icon.is_some() {
		return Err(Error::CustomIconHardLink {
			path: entry.destination.as_str().into(),
		});
	}
	Ok(())
}

pub(super) fn apply<'build>(
	prepared: &'build PreparedBuild,
	manifest_entry: &Entry,
	mut target: ManifestEntry<'build, Payload<'build>>,
	entries: &mut Vec<ManifestEntry<'build, Payload<'build>>>,
	date: u32,
) -> Result<ManifestEntry<'build, Payload<'build>>, Error> {
	let invalid_target = || Error::CustomIconTarget {
		path: manifest_entry.destination.as_str().into(),
	};
	let Some(fork) = prepared.item_icon(&manifest_entry.destination) else {
		return if manifest_entry.finder.custom_icon.is_some() {
			Err(invalid_target())
		} else {
			Ok(target)
		};
	};
	let fork = ForkSource::whole(Payload::Bytes(Cow::Borrowed(fork)));
	let child = prepared.item_icon_child(&manifest_entry.destination);
	match (&mut target.kind, child) {
		(EntryKind::File { resource, .. }, None) => {
			if resource.is_some() {
				return Err(Error::CustomIconResourceFork {
					path: manifest_entry.destination.as_str().into(),
				});
			}
			*resource = Some(fork);
		}
		(EntryKind::Folder, Some(child))
			if matches!(
				prepared.profile().filesystem(),
				FilesystemProfile::HfsplusUnicode | FilesystemProfile::Hfsx
			) =>
		{
			let mut metadata = generated_metadata(DEFAULT_FILE_MODE, date);
			metadata.finder_info = item_icon::folder_icon_file_finder_info();
			// The raw Icon\r name joins the ordinary catalog entries, whose
			// planner rejects collisions with an existing collation-equivalent path.
			entries.push(ManifestEntry {
				path: relative(child)?,
				metadata,
				kind: EntryKind::File {
					data: ForkSource::whole(Payload::Bytes(Cow::Borrowed(&[]))),
					resource: Some(fork),
				},
			});
		}
		_ => return Err(invalid_target()),
	}
	item_icon::mark_custom_icon(&mut target.metadata.finder_info);
	Ok(target)
}
