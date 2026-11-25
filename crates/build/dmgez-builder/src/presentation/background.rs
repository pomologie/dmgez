use std::{
	borrow::Cow,
	collections::BTreeMap,
	io::Cursor,
	mem::size_of,
	path::{Path, PathBuf},
};

use dmgez_config::{
	Background, BackgroundTemplate, Coordinate, FinderOptions, LocalPath, VirtualPath,
};
use dmgez_core::source::CapturedFile;
use dmgez_finder::{CatalogNames, ImageBackgroundIdentity, LayoutItem};

use crate::{
	manifest::{Manifest, SourceSnapshot},
	preflight::ExecutionProfile,
};

use super::{
	AssetBudget, Error, GeneratedAsset, PATH_SEPARATOR, PlannedStores, check_cancelled,
	parent_and_name,
};

mod budget;
mod icc;
mod output;
mod prebuilt;
mod retina;
mod template;

const DIRECTORY_NAME: &str = ".background";
const DIRECTORY_PATH: &str = "/.background";
const FILE_PREFIX: &str = "background-";
const PNG_EXTENSION: &str = ".png";
const TIFF_EXTENSION: &str = ".tiff";
const TIFF_TOP_LEFT: u16 = 1;
const RETINA_SCALE: u32 = 2;
// The single-leaf Finder writer produces an 8196-byte store. Cap each
// deferred binding separately, including its allocator bookkeeping.
const MAX_ENCODED_STORE_BYTES: usize = 16 << 10;
pub(super) const MAX_FILE_BYTES: usize = 32 << 20;
const MAX_PNG_DECODER_BYTES: usize = 8 << 20;
const MAX_RASTER_BYTES: usize = 64 << 20;
const MAX_IMAGE_DIMENSION: u32 = 8192;
const PNG_DECODER_BUDGET_DIVISOR: usize = 4;
const READ_CHUNK_BYTES: usize = 128 << 10;

/// Finder metadata awaiting the frozen catalog identity of its image.
#[derive(Debug)]
pub struct DeferredFinderStore {
	layout: FolderLayout,
	background_path: VirtualPath,
}

#[derive(Debug)]
struct FolderLayout {
	path: VirtualPath,
	options: FinderOptions,
	items: Vec<OwnedItem>,
	names: CatalogNames,
}

pub(super) struct PendingFinderStore {
	layout: FolderLayout,
	retained: usize,
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
		&self.layout.path
	}
	/// The on-image file that the store must reference.
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
		items.try_reserve_exact(self.layout.items.len())?;
		items.extend(self.layout.items.iter().map(|item| LayoutItem {
			name: &item.name,
			position: item.position,
		}));
		let bytes = self
			.layout
			.names
			.write_store_with_background_identity(&self.layout.options, &items, identity)
			.map_err(|source| Error::Layout {
				folder: parent_and_name(self.path()).0.to_owned(),
				source,
			})?;
		if bytes.capacity() > self.max_encoded_bytes() {
			return Err(Error::MetadataBudget);
		}
		Ok(bytes)
	}
}

impl PendingFinderStore {
	pub(super) fn plan(
		path: VirtualPath,
		options: Cow<'_, FinderOptions>,
		children: &[LayoutItem<'_>],
		names: CatalogNames,
		budget: &mut AssetBudget,
	) -> Result<Self, Error> {
		let retained = budget::deferred_metadata_bytes(&path, &options, children)
			.ok_or(Error::MetadataBudget)?;
		budget.charge(retained)?;
		budget.charge(MAX_ENCODED_STORE_BYTES)?;
		budget.charge(size_of::<(Self, usize)>())?;
		let mut items = Vec::new();
		items.try_reserve_exact(children.len())?;
		items.extend(children.iter().map(|child| OwnedItem {
			name: child.name.to_owned(),
			position: child.position,
		}));
		Ok(Self {
			layout: FolderLayout {
				path,
				options: options.into_owned(),
				items,
				names,
			},
			retained,
		})
	}

	pub(super) fn bind(
		self,
		background_path: &VirtualPath,
		budget: &mut AssetBudget,
	) -> Result<(DeferredFinderStore, usize), Error> {
		let path_bytes = background_path.as_str().len();
		budget.charge(path_bytes)?;
		let retained = self
			.retained
			.checked_add(path_bytes)
			.ok_or(Error::MetadataBudget)?;
		Ok((
			DeferredFinderStore {
				layout: self.layout,
				background_path: background_path.clone(),
			},
			retained,
		))
	}
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum BackgroundKey {
	Image(PathBuf, Option<PathBuf>),
	Template(BackgroundTemplate),
}

pub(super) enum BackgroundSource {
	Image {
		source: LocalPath,
		retina: Option<LocalPath>,
	},
	Template(BackgroundTemplate),
}

impl PlannedStores {
	pub(super) fn add_background(
		&mut self,
		background: &Background,
		folder: &str,
		manifest: &Manifest,
		by_source: &mut BTreeMap<BackgroundKey, usize>,
		budget: &mut AssetBudget,
	) -> Result<Option<usize>, Error> {
		let index = match background {
			Background::Image { source, retina } => {
				let key = BackgroundKey::Image(
					source.as_path().to_owned(),
					retina.as_ref().map(|path| path.as_path().to_owned()),
				);
				let path_bytes =
					source_path_bytes(source, retina.as_ref()).ok_or(Error::MetadataBudget)?;
				self.reserve_background(key, path_bytes, manifest, by_source, budget, || {
					BackgroundSource::Image {
						source: source.clone(),
						retina: retina.clone(),
					}
				})?
			}
			Background::Template { name } => {
				let template = name.as_str().parse().map_err(|_| Error::Layout {
					folder: folder.to_owned(),
					source: dmgez_finder::Error::TemplateBackground,
				})?;
				self.reserve_background(
					BackgroundKey::Template(template),
					0,
					manifest,
					by_source,
					budget,
					|| BackgroundSource::Template(template),
				)?
			}
			_ => return Ok(None),
		};
		Ok(Some(index))
	}

	fn reserve_background(
		&mut self,
		key: BackgroundKey,
		path_bytes: usize,
		manifest: &Manifest,
		by_source: &mut BTreeMap<BackgroundKey, usize>,
		budget: &mut AssetBudget,
		source: impl FnOnce() -> BackgroundSource,
	) -> Result<usize, Error> {
		if let Some(&index) = by_source.get(&key) {
			return Ok(index);
		}
		if self.backgrounds.is_empty() {
			reserve_directory(&mut self.assets.directories, manifest, budget)?;
		}
		let index = self.backgrounds.len();
		budget.reserve_assets(1)?;
		budget.charge(size_of::<BackgroundSource>() + size_of::<(BackgroundKey, usize)>())?;
		// The deduplication key and the capture plan each own the configured paths.
		budget.charge(path_bytes.checked_mul(2).ok_or(Error::MetadataBudget)?)?;
		self.backgrounds.try_reserve_exact(1)?;
		self.backgrounds.push(source());
		by_source.insert(key, index);
		Ok(index)
	}
}

fn source_path_bytes(source: &LocalPath, retina: Option<&LocalPath>) -> Option<usize> {
	source
		.as_path()
		.as_os_str()
		.len()
		.checked_add(retina.map_or(0, |path| path.as_path().as_os_str().len()))
}

fn reserve_directory(
	directories: &mut Vec<VirtualPath>,
	manifest: &Manifest,
	budget: &mut AssetBudget,
) -> Result<(), Error> {
	if let Some(entry) = manifest.entries().iter().find(|entry| {
		entry
			.destination
			.components()
			.next()
			.is_some_and(|name| name.eq_ignore_ascii_case(DIRECTORY_NAME))
	}) {
		return Err(Error::BackgroundCollision {
			path: entry.destination.clone(),
		});
	}
	budget.reserve_assets(1)?;
	budget.charge(DIRECTORY_PATH.len())?;
	directories.try_reserve_exact(1)?;
	directories.push(
		DIRECTORY_PATH
			.parse()
			.expect("the reserved background directory is a valid path"),
	);
	Ok(())
}

impl BackgroundSource {
	pub(super) fn capture(
		&self,
		index: usize,
		manifest: &mut Manifest,
		profile: &ExecutionProfile,
		config_dir: &Path,
		output: Option<&LocalPath>,
		budget: &mut AssetBudget,
	) -> Result<GeneratedAsset, Error> {
		check_cancelled(profile)?;
		let (bytes, extension) = match self {
			Self::Image { source, retina } => capture_files(
				source,
				retina.as_ref(),
				manifest,
				profile,
				config_dir,
				output,
				budget,
			)?,
			Self::Template(BackgroundTemplate::BuiltinArrow) => {
				let bytes = template::render(profile, budget.metadata_bytes)?;
				budget.charge(bytes.capacity())?;
				(bytes, TIFF_EXTENSION)
			}
		};
		check_cancelled(profile)?;
		let path = format!("{DIRECTORY_PATH}{PATH_SEPARATOR}{FILE_PREFIX}{index}{extension}");
		budget.charge(path.len())?;
		Ok(GeneratedAsset {
			path: path
				.parse()
				.expect("generated background paths use printable ASCII components"),
			bytes,
		})
	}
}

fn capture_files(
	source: &LocalPath,
	retina: Option<&LocalPath>,
	manifest: &mut Manifest,
	profile: &ExecutionProfile,
	config_dir: &Path,
	output: Option<&LocalPath>,
	budget: &mut AssetBudget,
) -> Result<(Vec<u8>, &'static str), Error> {
	let base = capture_source(source, manifest, profile, config_dir, output, budget)?;
	let retina = retina
		.map(|path| capture_source(path, manifest, profile, config_dir, output, budget))
		.transpose()?;
	let base = manifest.auxiliary_source(base);
	let available = profile.write_options().max_buffer_bytes;
	let bytes = read_source(base, profile, available)?;
	let available = available - bytes.capacity();
	let result = if let Some(retina) = retina {
		let retina = manifest.auxiliary_source(retina);
		let retina_bytes = read_source(retina, profile, available)?;
		let available = available - retina_bytes.capacity();
		let encoded = retina::encode(
			&bytes,
			&retina_bytes,
			available,
			budget.metadata_bytes,
			profile,
		)?;
		budget.charge(encoded.capacity())?;
		retina.validate().map_err(Error::BackgroundSource)?;
		(encoded, TIFF_EXTENSION)
	} else {
		budget.charge(bytes.capacity())?;
		let extension = if prebuilt::is_tiff(&bytes) {
			prebuilt::validate(&bytes, available, profile)?;
			TIFF_EXTENSION
		} else {
			validate_png(&bytes, available)?;
			PNG_EXTENSION
		};
		(bytes, extension)
	};
	check_cancelled(profile)?;
	base.validate().map_err(Error::BackgroundSource)?;
	Ok(result)
}

fn capture_source(
	input: &LocalPath,
	manifest: &mut Manifest,
	profile: &ExecutionProfile,
	config_dir: &Path,
	output: Option<&LocalPath>,
	budget: &mut AssetBudget,
) -> Result<usize, Error> {
	check_cancelled(profile)?;
	let metadata_before = manifest.metadata_bytes();
	let index = manifest
		.capture_auxiliary_regular(
			input,
			config_dir,
			output,
			profile.limits().max_metadata_bytes as u64,
		)
		.map_err(Error::BackgroundSource)?;
	let captured_bytes = manifest.metadata_bytes() - metadata_before;
	budget.charge(usize::try_from(captured_bytes).map_err(|_| Error::MetadataBudget)?)?;
	Ok(index)
}

fn read_source(
	source: &SourceSnapshot,
	profile: &ExecutionProfile,
	available: usize,
) -> Result<Vec<u8>, Error> {
	let size = usize::try_from(source.metadata().size)
		.ok()
		.filter(|&size| size > 0 && size <= MAX_FILE_BYTES)
		.ok_or(Error::BackgroundFileSize)?;
	if size >= available {
		return Err(Error::BackgroundMemory);
	}
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(size)?;
	if bytes.capacity() >= available {
		return Err(Error::BackgroundMemory);
	}
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
	Ok(bytes)
}

fn validate_png(bytes: &[u8], available: usize) -> Result<(), Error> {
	// Reserve decoder-owned memory separately from the caller's raster buffer.
	let decoder_bytes = (available / PNG_DECODER_BUDGET_DIVISOR).min(MAX_PNG_DECODER_BYTES);
	let raster_bytes = (available - decoder_bytes).min(MAX_RASTER_BYTES);
	let limits = png::Limits {
		bytes: decoder_bytes,
	};
	let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
	decoder.set_ignore_text_chunk(true);
	decoder.set_ignore_iccp_chunk(true);
	let header = decoder.read_header_info().map_err(Error::BackgroundPng)?;
	if header.width > MAX_IMAGE_DIMENSION || header.height > MAX_IMAGE_DIMENSION {
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
