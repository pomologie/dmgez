use std::{collections::BTreeMap, path::Path};

use dmgez_config::{IconSlot, LocalPath, VirtualPath};
use dmgez_core::source::CapturedFile;
use dmgez_finder::volume_icon::{self, MAX_BYTES, NAME};

use crate::{
	manifest::{self, Manifest, SourceSnapshot},
	preflight::ExecutionProfile,
};

use super::{AssetBudget, Error, GeneratedAsset, PATH_SEPARATOR, check_cancelled};

const READ_CHUNK_BYTES: usize = 128 << 10;
// Both slot maps contain at most ten entries. Reserve their nodes separately
// from PNG buffers, which are charged at actual allocation capacity.
const ICON_MAP_WORKSPACE_BYTES: usize = 4 << 10;
pub(crate) const FINDER_INFO_BYTES: usize = 32;

/// Flag updates to merge into the root catalog directory's Finder metadata.
#[derive(Debug, Default)]
pub(crate) struct RootFinderInfo([u8; FINDER_INFO_BYTES]);

impl RootFinderInfo {
	pub(crate) fn apply(&self, finder_info: &mut [u8; FINDER_INFO_BYTES]) {
		for (out, flag) in finder_info.iter_mut().zip(self.0) {
			*out |= flag;
		}
	}
}

pub(crate) enum IconSources {
	Prebuilt(usize),
	Images(BTreeMap<IconSlot, usize>),
}

pub(crate) fn capture_sources(
	manifest: &mut Manifest,
	profile: &ExecutionProfile,
	config_dir: &Path,
	output: Option<&LocalPath>,
) -> Result<Option<IconSources>, manifest::Error> {
	let mut capture = |input: &LocalPath| {
		if profile.write_options().cancellation.is_cancelled() {
			return Err(manifest::Error::Cancelled);
		}
		manifest.capture_auxiliary_regular(
			input,
			config_dir,
			output,
			profile.limits().max_metadata_bytes as u64,
		)
	};
	if let Some(source) = profile.volume_icon_source() {
		return capture(source).map(|index| Some(IconSources::Prebuilt(index)));
	}
	let Some(images) = profile.volume_icon_images() else {
		return Ok(None);
	};
	let indices = images
		.iter()
		.map(|(&slot, input)| capture(input).map(|index| (slot, index)))
		.collect::<Result<_, _>>()?;
	Ok(Some(IconSources::Images(indices)))
}

pub(crate) fn prepare_asset(
	sources: IconSources,
	manifest: &Manifest,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
) -> Result<(GeneratedAsset, RootFinderInfo), Error> {
	check_cancelled(profile)?;
	let path: VirtualPath = format!("{PATH_SEPARATOR}{NAME}")
		.parse()
		.expect("the reserved root icon name is a valid virtual path");
	if let Some(entry) = manifest.entries().iter().find(|entry| {
		entry
			.destination
			.as_str()
			.eq_ignore_ascii_case(path.as_str())
	}) {
		return Err(Error::VolumeIconCollision {
			path: entry.destination.clone(),
		});
	}
	budget.reserve_assets(1)?;
	budget.charge(path.as_str().len())?;
	let bytes = match sources {
		IconSources::Prebuilt(index) => {
			read_prebuilt(manifest.auxiliary_source(index), profile, budget)?
		}
		IconSources::Images(indices) => encode_pngs(indices, manifest, profile, budget)?,
	};
	let mut flags = RootFinderInfo::default();
	volume_icon::mark_root_finder_info(&mut flags.0);
	Ok((GeneratedAsset { path, bytes }, flags))
}

fn source_size(source: &SourceSnapshot) -> Result<usize, Error> {
	usize::try_from(source.metadata().size)
		.ok()
		.filter(|&size| size <= MAX_BYTES)
		.ok_or(Error::VolumeIcon(volume_icon::Error::Size))
}

fn read_prebuilt(
	source: &SourceSnapshot,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
) -> Result<Vec<u8>, Error> {
	let size = source_size(source)?;
	let scratch = volume_icon::validation_memory(size).ok_or(Error::VolumeIconMemory)?;
	if scratch > profile.write_options().max_buffer_bytes {
		return Err(Error::VolumeIconMemory);
	}
	budget.charge(size)?;
	let bytes = read_bytes(source, profile, size, MAX_BYTES)?;
	budget.charge(bytes.capacity() - size)?;
	volume_icon::validate(&bytes)?;
	check_cancelled(profile)?;
	source.validate()?;
	Ok(bytes)
}

fn encode_pngs(
	indices: BTreeMap<IconSlot, usize>,
	manifest: &Manifest,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
) -> Result<Vec<u8>, Error> {
	let workspace = profile
		.write_options()
		.max_buffer_bytes
		.checked_sub(ICON_MAP_WORKSPACE_BYTES)
		.ok_or(Error::VolumeIconMemory)?;
	let mut retained = 0;
	let mut largest = 0;
	let mut pngs = BTreeMap::new();
	for (&slot, &index) in &indices {
		let source = manifest.auxiliary_source(index);
		let size = source_size(source)?;
		// Each allocation was bounded by the remaining encoded input cap.
		let encoded_limit = MAX_BYTES - retained;
		if size > encoded_limit {
			return Err(volume_icon::Error::Size.into());
		}
		largest = largest.max(size);
		let scratch = volume_icon::png_encoding_memory(largest).ok_or(Error::VolumeIconMemory)?;
		let available = workspace
			.checked_sub(scratch)
			.and_then(|remaining| remaining.checked_sub(retained))
			.ok_or(Error::VolumeIconMemory)?;
		let bytes = read_bytes(source, profile, size, available.min(encoded_limit))?;
		retained += bytes.capacity();
		pngs.insert(slot, bytes);
	}
	check_cancelled(profile)?;
	let bytes =
		volume_icon::encode_pngs(&pngs, budget.metadata_bytes).map_err(|error| match error {
			volume_icon::Error::OutputBudget => Error::MetadataBudget,
			other => Error::VolumeIcon(other),
		})?;
	budget.charge(bytes.capacity())?;
	for index in indices.into_values() {
		check_cancelled(profile)?;
		manifest.auxiliary_source(index).validate()?;
	}
	Ok(bytes)
}

fn read_bytes(
	source: &SourceSnapshot,
	profile: &ExecutionProfile,
	size: usize,
	capacity_limit: usize,
) -> Result<Vec<u8>, Error> {
	check_cancelled(profile)?;
	if size > capacity_limit {
		return Err(Error::VolumeIconMemory);
	}
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(size)?;
	if bytes.capacity() > capacity_limit {
		return Err(Error::VolumeIconMemory);
	}
	bytes.resize(size, 0);

	let file = CapturedFile::new(source.open_regular()?)?;
	let mut offset = 0;
	for chunk in bytes.chunks_mut(READ_CHUNK_BYTES) {
		check_cancelled(profile)?;
		file.read_exact_at(offset, chunk)?;
		offset += chunk.len() as u64;
	}
	source.validate()?;
	Ok(bytes)
}
