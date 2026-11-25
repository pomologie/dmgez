use std::{collections::BTreeMap, mem::size_of, path::Path};

use dmgez_config::{LocalPath, VirtualPath};
use dmgez_core::source::CapturedFile;
use dmgez_finder::{item_icon, volume_icon};

use crate::{
	manifest::{Entry, EntryKind, Manifest, SourceSnapshot},
	preflight::{ExecutionProfile, FilesystemProfile},
};

use super::{AssetBudget, Error, PATH_SEPARATOR, check_cancelled, volume_icon::read_bytes};

mod signed_macho;

const APP_BUNDLE_SUFFIX: [u8; 4] = *b".app";
const SIGNATURE_DIRECTORY: &str = "_CodeSignature";
const SIGNATURE_RESOURCES: &str = "CodeResources";
const BUNDLE_CONTENTS: &str = "Contents";
const FRAMEWORK_VERSIONS: &str = "Versions";

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
	let signatures = entries.iter().filter_map(signed_bundle_root);
	let mut roots = Vec::new();
	let count = signatures.clone().count();
	let check_budget = |capacity: usize| {
		capacity
			.checked_mul(size_of::<&str>())
			.filter(|&bytes| bytes <= budget.metadata_bytes)
			.ok_or(Error::MetadataBudget)
	};
	check_budget(count)?;
	roots.try_reserve_exact(count)?;
	check_budget(roots.capacity())?;
	roots.extend(signatures);
	roots.sort_unstable_by(|left, right| folded(left).cmp(folded(right)));

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
		let app_path = entry
			.destination
			.components()
			.any(|part| has_app_suffix(part.as_bytes()))
			|| entry.source.as_ref().is_some_and(|source| {
				source
					.path()
					.components()
					.any(|part| has_app_suffix(part.as_os_str().as_encoded_bytes()))
			});
		if !eligible_kind {
			return Err(unsupported_target(&entry.destination));
		}
		if app_path
			|| inside_signed_bundle(&entry.destination, &roots)
			|| has_source_bundle_marker(entry)?
		{
			return Err(Error::ItemIconSignedBundle {
				path: entry.destination.clone(),
			});
		}
		if matches!(entry.kind, EntryKind::File)
			&& let Some(source) = &entry.source
			&& has_executable_signature(source, &entry.destination)?
		{
			return Err(Error::ItemIconSignedExecutable {
				path: entry.destination.clone(),
			});
		}
	}
	Ok(())
}

fn has_executable_signature(
	source: &SourceSnapshot,
	destination: &VirtualPath,
) -> Result<bool, Error> {
	let read_error = |source| Error::ItemIconTargetRead {
		path: destination.clone(),
		source,
	};
	let file = CapturedFile::new(source.open_regular().map_err(Error::ItemIconSource)?)
		.map_err(read_error)?;
	let signed = signed_macho::has_or_may_have_signature(&file).map_err(read_error)?;
	source.validate().map_err(Error::ItemIconSource)?;
	Ok(signed)
}

fn has_source_bundle_marker(entry: &Entry) -> Result<bool, Error> {
	match entry.source.as_ref() {
		Some(source) => source
			.has_signed_bundle_marker()
			.map_err(Error::ItemIconSource),
		None => Ok(false),
	}
}

fn has_app_suffix(name: &[u8]) -> bool {
	name.last_chunk::<{ APP_BUNDLE_SUFFIX.len() }>()
		.is_some_and(|suffix| suffix.eq_ignore_ascii_case(&APP_BUNDLE_SUFFIX))
}

fn signed_bundle_root(entry: &Entry) -> Option<&str> {
	if !matches!(entry.kind, EntryKind::File) {
		return None;
	}
	let (directory, resource) = entry.destination.as_str().rsplit_once(PATH_SEPARATOR)?;
	if !resource.eq_ignore_ascii_case(SIGNATURE_RESOURCES) {
		return None;
	}
	let (container, signature) = directory.rsplit_once(PATH_SEPARATOR)?;
	if !signature.eq_ignore_ascii_case(SIGNATURE_DIRECTORY) {
		return None;
	}
	let Some((parent, name)) = container.rsplit_once(PATH_SEPARATOR) else {
		return Some(container);
	};
	if name.eq_ignore_ascii_case(BUNDLE_CONTENTS) {
		return Some(parent);
	}
	if let Some((root, directory)) = parent.rsplit_once(PATH_SEPARATOR)
		&& directory.eq_ignore_ascii_case(FRAMEWORK_VERSIONS)
	{
		return Some(root);
	}
	Some(container)
}

fn inside_signed_bundle(path: &VirtualPath, roots: &[&str]) -> bool {
	let path = path.as_str();
	std::iter::once(path)
		.chain(
			path.rmatch_indices(PATH_SEPARATOR)
				.map(|(end, _)| &path[..end]),
		)
		.any(|component| {
			roots
				.binary_search_by(|root| folded(root).cmp(folded(component)))
				.is_ok()
		})
}

fn folded(path: &str) -> impl Iterator<Item = u8> + '_ {
	path.bytes().map(|byte| byte.to_ascii_lowercase())
}

fn unsupported_target(destination: &VirtualPath) -> Error {
	Error::ItemIconTarget {
		path: destination.clone(),
	}
}
