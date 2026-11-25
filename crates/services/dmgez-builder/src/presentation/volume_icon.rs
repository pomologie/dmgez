use dmgez_core::source::CapturedFile;
use dmgez_finder::volume_icon::{self, MAX_BYTES, NAME};

use crate::{
	manifest::{Manifest, SourceSnapshot},
	preflight::ExecutionProfile,
};

use super::{AssetBudget, Error, GeneratedAsset, PATH_SEPARATOR, check_cancelled};

const READ_CHUNK_BYTES: usize = 128 << 10;
const PNG_DECODER_BYTES: usize = 64 << 20;
const MAX_RGBA_RASTER_BYTES: usize = 4 << 20;
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

pub(crate) fn prepare_asset(
	source: &SourceSnapshot,
	manifest: &Manifest,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
) -> Result<(GeneratedAsset, RootFinderInfo), Error> {
	check_cancelled(profile)?;
	let path: dmgez_config::VirtualPath = format!("{PATH_SEPARATOR}{NAME}")
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
	let size = usize::try_from(source.metadata().size)
		.ok()
		.filter(|&size| size <= MAX_BYTES)
		.ok_or(volume_icon::Error::Size)?;
	// Use the writer's otherwise idle budget for decoder workspace, the
	// largest raster, and the decoder's temporary copy of one element.
	let validation_bytes = PNG_DECODER_BYTES + MAX_RGBA_RASTER_BYTES + size;
	if validation_bytes > profile.write_options().max_buffer_bytes {
		return Err(Error::VolumeIconMemory);
	}

	budget.reserve_assets(1)?;
	budget.charge(path.as_str().len())?;
	budget.charge(size)?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(size)?;
	budget.charge(bytes.capacity() - size)?;
	bytes.resize(size, 0);

	let file = CapturedFile::new(source.open_regular()?)?;
	let mut offset = 0;
	for chunk in bytes.chunks_mut(READ_CHUNK_BYTES) {
		check_cancelled(profile)?;
		file.read_exact_at(offset, chunk)?;
		offset += chunk.len() as u64;
	}
	source.validate()?;
	let icon = volume_icon::validate(&bytes)?;
	let mut flags = RootFinderInfo::default();
	icon.mark_root_finder_info(&mut flags.0);
	check_cancelled(profile)?;
	source.validate()?;
	Ok((GeneratedAsset { path, bytes }, flags))
}
