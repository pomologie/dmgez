use std::{collections::BTreeMap, mem::size_of, path::Path};

use dmgez_config::{LocalPath, VirtualPath};
use dmgez_finder::{item_icon, volume_icon};

use crate::{
	manifest::{Entry, EntryKind, Manifest, SourceSnapshot},
	preflight::{ExecutionProfile, FilesystemProfile},
};

use super::{
	AssetBudget, Error, PATH_SEPARATOR, check_cancelled,
	signed_targets::{self, Protection, SignedTargets},
	volume_icon::read_bytes,
};

#[derive(Debug, Default)]
pub(crate) struct ItemIcons {
	entries: BTreeMap<VirtualPath, ItemIcon>,
	metadata_bytes: usize,
}

#[derive(Debug)]
struct ItemIcon {
	resource_fork: Vec<u8>,
	folder_child: Option<VirtualPath>,
}

impl ItemIcons {
	pub(crate) fn resource_fork(&self, destination: &VirtualPath) -> Option<&[u8]> {
		Some(&self.entries.get(destination)?.resource_fork)
	}

	pub(crate) fn folder_child(&self, destination: &VirtualPath) -> Option<&VirtualPath> {
		self.entries.get(destination)?.folder_child.as_ref()
	}

	pub(crate) fn len(&self) -> usize {
		self.entries.len()
	}

	pub(crate) const fn metadata_bytes(&self) -> usize {
		self.metadata_bytes
	}
}

pub(super) fn prepare(
	manifest: &mut Manifest,
	profile: &ExecutionProfile,
	config_dir: &Path,
	output: Option<&LocalPath>,
	budget: &mut AssetBudget,
) -> Result<ItemIcons, Error> {
	let mut prepared = ItemIcons::default();
	if !manifest
		.entries()
		.iter()
		.any(|entry| entry.finder.custom_icon.is_some())
	{
		return Ok(prepared);
	}
	validate_targets(manifest.entries(), profile, budget)?;

	for index in 0..manifest.entries().len() {
		let entry = &manifest.entries()[index];
		let Some(input) = entry.finder.custom_icon.clone() else {
			continue;
		};
		check_cancelled(profile)?;
		let destination = entry.destination.clone();
		let folder_child = if matches!(entry.kind, EntryKind::Directory) {
			budget.reserve_assets(1)?;
			Some(
				format!("{destination}{PATH_SEPARATOR}{}", item_icon::ICON_FILE_NAME,)
					.parse()
					.map_err(|_| unsupported_target(&destination))?,
			)
		} else {
			None
		};
		let owned_bytes = size_of::<(VirtualPath, ItemIcon)>()
			.checked_add(destination.as_str().len())
			.and_then(|bytes| {
				bytes.checked_add(
					folder_child
						.as_ref()
						.map_or(0, |path: &VirtualPath| path.as_str().len()),
				)
			})
			.ok_or(Error::MetadataBudget)?;
		budget.charge(owned_bytes)?;

		let metadata_before = manifest.metadata_bytes();
		let source = manifest
			.capture_auxiliary_regular(
				&input,
				config_dir,
				output,
				profile.limits().max_metadata_bytes as u64,
			)
			.map_err(Error::ItemIconSource)?;
		let source_metadata_bytes = usize::try_from(manifest.metadata_bytes() - metadata_before)
			.map_err(|_| Error::MetadataBudget)?;
		budget.charge(source_metadata_bytes)?;
		let resource_fork = encode(
			manifest.auxiliary_source(source),
			profile,
			budget,
			&destination,
		)?;
		budget.charge(resource_fork.capacity())?;
		prepared.metadata_bytes = prepared
			.metadata_bytes
			.checked_add(owned_bytes)
			.and_then(|bytes| bytes.checked_add(resource_fork.capacity()))
			.ok_or(Error::MetadataBudget)?;
		prepared.entries.insert(
			destination,
			ItemIcon {
				resource_fork,
				folder_child,
			},
		);
	}
	Ok(prepared)
}

fn encode(
	source: &SourceSnapshot,
	profile: &ExecutionProfile,
	budget: &AssetBudget,
	destination: &VirtualPath,
) -> Result<Vec<u8>, Error> {
	let size = usize::try_from(source.metadata().size)
		.ok()
		.filter(|&size| size <= volume_icon::MAX_BYTES)
		.ok_or_else(|| Error::ItemIcon {
			path: destination.clone(),
			source: item_icon::Error::Size,
		})?;
	let scratch = volume_icon::validation_memory(size).ok_or(Error::ItemIconMemory)?;
	let available = profile
		.write_options()
		.max_buffer_bytes
		.checked_sub(scratch)
		.ok_or(Error::ItemIconMemory)?;
	let bytes = read_bytes(source, profile, size, available.min(volume_icon::MAX_BYTES)).map_err(
		|error| match error {
			Error::VolumeIconMemory => Error::ItemIconMemory,
			Error::VolumeIconSource(source) => Error::ItemIconSource(source),
			Error::VolumeIconRead(source) => Error::ItemIconRead(source),
			other => other,
		},
	)?;
	check_cancelled(profile)?;
	let fork =
		item_icon::encode_resource_fork(&bytes, budget.metadata_bytes).map_err(
			|error| match error {
				item_icon::Error::OutputBudget => Error::MetadataBudget,
				source => Error::ItemIcon {
					path: destination.clone(),
					source,
				},
			},
		)?;
	check_cancelled(profile)?;
	source.validate().map_err(Error::ItemIconSource)?;
	Ok(fork)
}

fn validate_targets(
	entries: &[Entry],
	profile: &ExecutionProfile,
	budget: &AssetBudget,
) -> Result<(), Error> {
	let targets =
		SignedTargets::new(entries, budget.metadata_bytes).map_err(|error| match error {
			signed_targets::Error::Budget => Error::MetadataBudget,
			signed_targets::Error::Reserve(error) => Error::Reserve(error),
			error => Error::SignedTargets(error),
		})?;

	for entry in entries
		.iter()
		.filter(|entry| entry.finder.custom_icon.is_some())
	{
		check_cancelled(profile)?;
		let eligible_kind = match entry.kind {
			EntryKind::File => true,
			EntryKind::Directory => profile.filesystem() != FilesystemProfile::HfsplusAscii,
			EntryKind::Symlink(_) => false,
		};
		if !eligible_kind {
			return Err(unsupported_target(&entry.destination));
		}
		let protection = targets.protection(entry).map_err(|error| match error {
			signed_targets::Error::Source(error) => Error::ItemIconSource(error),
			signed_targets::Error::Read(source) => Error::ItemIconTargetRead {
				path: entry.destination.clone(),
				source,
			},
			error => Error::SignedTargets(error),
		})?;
		match protection {
			Some(Protection::Bundle) => {
				return Err(Error::ItemIconSignedBundle {
					path: entry.destination.clone(),
				});
			}
			Some(Protection::Executable) => {
				return Err(Error::ItemIconSignedExecutable {
					path: entry.destination.clone(),
				});
			}
			None => {}
		}
	}
	Ok(())
}

fn unsupported_target(destination: &VirtualPath) -> Error {
	Error::ItemIconTarget {
		path: destination.clone(),
	}
}
