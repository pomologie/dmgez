use std::{borrow::Cow, io::Cursor, mem::size_of, path::Path};

use dmgez_config::{
	Background, Column, Coordinate, Dimension, Direction, FinderOptions, LocalPath, VirtualPath,
};
use dmgez_core::source::CapturedFile;
use dmgez_finder::{ImageBackgroundIdentity, LayoutItem, write_store_with_background_identity};

use crate::{
	manifest::{Manifest, SourceSnapshot},
	preflight::ExecutionProfile,
};

use super::{AssetBudget, Error, GeneratedAsset, PATH_SEPARATOR, check_cancelled, parent_and_name};

pub(super) const DIRECTORY_NAME: &str = ".background";
pub(super) const DIRECTORY_PATH: &str = "/.background";
const FILE_PREFIX: &str = "background-";
const PNG_EXTENSION: &str = ".png";
// The single-leaf Finder writer produces an 8196-byte store. Cap each
// deferred binding separately, including its allocator bookkeeping.
const MAX_ENCODED_STORE_BYTES: usize = 16 << 10;
pub(super) const MAX_FILE_BYTES: usize = 32 << 20;
const MAX_PNG_DECODER_BYTES: usize = 8 << 20;
const MAX_PNG_RASTER_BYTES: usize = 64 << 20;
const MAX_PNG_DIMENSION: u32 = 8192;
const PNG_DECODER_BUDGET_DIVISOR: usize = 4;
const READ_CHUNK_BYTES: usize = 128 << 10;
const MINIMUM_COLUMN_WIDTH: u8 = 0;

/// Finder metadata awaiting the frozen catalog identity of its image.
#[derive(Debug)]
pub struct DeferredFinderStore {
	path: VirtualPath,
	background_path: VirtualPath,
	options: FinderOptions,
	items: Vec<OwnedItem>,
}

#[derive(Debug)]
struct OwnedItem {
	name: String,
	position: Option<[Coordinate; 2]>,
}

impl DeferredFinderStore {
	/// The file path to reserve before the catalog identity pass.
	#[must_use]
	pub const fn path(&self) -> &VirtualPath {
		&self.path
	}
	/// The already captured image file that the store must reference.
	#[must_use]
	pub const fn background_path(&self) -> &VirtualPath {
		&self.background_path
	}

	/// Upper bound for the retained byte buffer when binding this store.
	#[must_use]
	pub const fn max_encoded_bytes(&self) -> usize {
		MAX_ENCODED_STORE_BYTES
	}

	/// Creates the store after the filesystem assigns its final identities.
	/// # Errors
	/// Rejects a mismatched target path, invalid alias/layout, or oversized store.
	pub fn render(&self, identity: ImageBackgroundIdentity<'_>) -> Result<Vec<u8>, Error> {
		if identity.volume_relative_path != self.background_path.as_str() {
			return Err(Error::BackgroundTargetMismatch);
		}
		let mut items = Vec::new();
		items.try_reserve_exact(self.items.len())?;
		items.extend(self.items.iter().map(|item| LayoutItem {
			name: &item.name,
			position: item.position,
		}));
		let bytes = write_store_with_background_identity(&self.options, &items, identity).map_err(
			|source| Error::Layout {
				folder: parent_and_name(&self.path).0.to_owned(),
				source,
			},
		)?;
		if bytes.capacity() > self.max_encoded_bytes() {
			return Err(Error::MetadataBudget);
		}
		Ok(bytes)
	}

	pub(super) fn plan(
		path: VirtualPath,
		background_path: &VirtualPath,
		options: Cow<'_, FinderOptions>,
		children: &[LayoutItem<'_>],
		budget: &mut AssetBudget,
	) -> Result<(Self, usize), Error> {
		let retained = deferred_metadata_bytes(&path, background_path, &options, children)
			.ok_or(Error::MetadataBudget)?;
		budget.charge(retained)?;
		budget.charge(MAX_ENCODED_STORE_BYTES)?;
		let mut items = Vec::new();
		items.try_reserve_exact(children.len())?;
		items.extend(children.iter().map(|child| OwnedItem {
			name: child.name.to_owned(),
			position: child.position,
		}));
		Ok((
			Self {
				path,
				background_path: background_path.clone(),
				options: options.into_owned(),
				items,
			},
			retained,
		))
	}
}

fn deferred_metadata_bytes(
	path: &VirtualPath,
	background_path: &VirtualPath,
	options: &FinderOptions,
	children: &[LayoutItem<'_>],
) -> Option<usize> {
	let mut bytes = size_of::<DeferredFinderStore>().checked_add(path.as_str().len())?;
	bytes = bytes.checked_add(background_path.as_str().len())?;
	if let Some(Background::Image { source, .. }) = &options.background {
		bytes = bytes.checked_add(source.as_path().as_os_str().len())?;
	}
	if let Some(icons) = &options.icon_view {
		for name in icons.positions.keys() {
			bytes = bytes
				.checked_add(size_of::<(String, [Coordinate; 2])>())?
				.checked_add(name.len())?;
		}
	}
	if let Some(list) = &options.list_view {
		if let Some(columns) = &list.columns {
			bytes = bytes.checked_add(size_of::<Column>().checked_mul(columns.len())?)?;
		}
		bytes = bytes.checked_add(
			size_of::<(Column, Dimension<MINIMUM_COLUMN_WIDTH>)>()
				.checked_mul(list.column_widths.len())?,
		)?;
		bytes = bytes.checked_add(
			size_of::<(Column, Direction)>().checked_mul(list.sort_directions.len())?,
		)?;
	}
	for item in children {
		bytes = bytes
			.checked_add(size_of::<OwnedItem>() + size_of::<LayoutItem<'_>>())?
			.checked_add(item.name.len())?;
	}
	Some(bytes)
}

pub(super) struct BackgroundSource {
	pub(super) source: LocalPath,
	pub(super) path: VirtualPath,
}

impl BackgroundSource {
	pub(super) fn reserve(
		source: &LocalPath,
		index: usize,
		budget: &mut AssetBudget,
	) -> Result<Self, Error> {
		let path = format!("{DIRECTORY_PATH}{PATH_SEPARATOR}{FILE_PREFIX}{index}{PNG_EXTENSION}");
		budget.reserve_assets(1)?;
		budget.charge(path.len())?;
		budget.charge(source.as_path().as_os_str().len())?;
		Ok(Self {
			source: source.clone(),
			path: path
				.parse()
				.expect("generated PNG paths use printable ASCII components"),
		})
	}

	pub(super) fn capture(
		self,
		manifest: &mut Manifest,
		profile: &ExecutionProfile,
		config_dir: &Path,
		output: Option<&LocalPath>,
		budget: &mut AssetBudget,
	) -> Result<GeneratedAsset, Error> {
		check_cancelled(profile)?;
		let metadata_before = manifest.metadata_bytes();
		let index = manifest
			.capture_auxiliary_regular(
				&self.source,
				config_dir,
				output,
				profile.limits().max_metadata_bytes as u64,
			)
			.map_err(Error::BackgroundSource)?;
		let captured_bytes = manifest.metadata_bytes() - metadata_before;
		budget.charge(usize::try_from(captured_bytes).map_err(|_| Error::MetadataBudget)?)?;
		let bytes = read_image(manifest.auxiliary_source(index), profile, budget)?;
		Ok(GeneratedAsset {
			path: self.path,
			bytes,
		})
	}
}

fn read_image(
	source: &SourceSnapshot,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
) -> Result<Vec<u8>, Error> {
	let size = usize::try_from(source.metadata().size)
		.ok()
		.filter(|&size| size > 0 && size <= MAX_FILE_BYTES)
		.ok_or(Error::BackgroundFileSize)?;
	let available = profile
		.write_options()
		.max_buffer_bytes
		.checked_sub(size)
		.filter(|&bytes| bytes > 0)
		.ok_or(Error::BackgroundMemory)?;
	budget.charge(size)?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(size)?;
	let spare = bytes.capacity() - size;
	budget.charge(spare)?;
	let available = available
		.checked_sub(spare)
		.filter(|&bytes| bytes > 0)
		.ok_or(Error::BackgroundMemory)?;
	bytes.resize(size, 0);
	let file = CapturedFile::new(source.open_regular().map_err(Error::BackgroundSource)?)
		.map_err(Error::BackgroundRead)?;
	for (index, chunk) in bytes.chunks_mut(READ_CHUNK_BYTES).enumerate() {
		check_cancelled(profile)?;
		let offset = (index * READ_CHUNK_BYTES) as u64;
		file.read_exact_at(offset, chunk)
			.map_err(Error::BackgroundRead)?;
	}
	source.validate().map_err(Error::BackgroundSource)?;
	validate_png(&bytes, available)?;
	check_cancelled(profile)?;
	source.validate().map_err(Error::BackgroundSource)?;
	Ok(bytes)
}

fn validate_png(bytes: &[u8], available: usize) -> Result<(), Error> {
	// Reserve decoder-owned memory separately from the caller's raster buffer.
	let decoder_bytes = (available / PNG_DECODER_BUDGET_DIVISOR).min(MAX_PNG_DECODER_BYTES);
	let raster_bytes = (available - decoder_bytes).min(MAX_PNG_RASTER_BYTES);
	let limits = png::Limits {
		bytes: decoder_bytes,
	};
	let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
	decoder.set_ignore_text_chunk(true);
	decoder.set_ignore_iccp_chunk(true);
	let header = decoder.read_header_info().map_err(Error::BackgroundPng)?;
	if header.width > MAX_PNG_DIMENSION || header.height > MAX_PNG_DIMENSION {
		return Err(Error::BackgroundMemory);
	}
	let mut reader = decoder.read_info().map_err(Error::BackgroundPng)?;
	if reader.info().animation_control.is_some() {
		return Err(Error::BackgroundAnimated);
	}
	let size = reader
		.output_buffer_size()
		.filter(|&size| size <= raster_bytes)
		.ok_or(Error::BackgroundMemory)?;
	let mut raster = Vec::new();
	raster.try_reserve_exact(size)?;
	if raster.capacity() > raster_bytes {
		return Err(Error::BackgroundMemory);
	}
	raster.resize(size, 0);
	reader
		.next_frame(&mut raster)
		.map_err(Error::BackgroundPng)?;
	reader.finish().map_err(Error::BackgroundPng)?;
	Ok(())
}
