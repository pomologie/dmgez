//! Planning of Finder metadata and auxiliary assets for a resolved image tree.

use std::{
	borrow::Cow,
	collections::{BTreeMap, BTreeSet},
	mem::size_of,
	path::Path,
};

use dmgez_config::{FinderOptions, LocalPath, Project, VirtualPath, VirtualPathError};
use dmgez_finder::{LayoutItem, STORE_NAME, write_store};
use thiserror::Error;

use crate::{
	manifest::{EntryKind, Manifest},
	preflight::{ExecutionProfile, FinderPlan},
};

mod background;
pub(crate) mod settings;
#[cfg(feature = "volume-icon")]
pub(crate) mod volume_icon;

pub use background::DeferredFinderStore;
use background::{BackgroundSource, PendingFinderStore};
pub use dmgez_finder::ImageBackgroundIdentity;
use settings::{resolve_options, without_positions};

const IMAGE_ROOT: &str = "/";
const PATH_SEPARATOR: char = '/';

/// A reserved image path and its prepared Finder asset payload.
#[derive(Debug, Eq, PartialEq)]
pub struct GeneratedAsset {
	/// Path to reserve in the image draft.
	pub path: VirtualPath,
	/// Exact data to bind to the reserved file.
	pub bytes: Vec<u8>,
}

/// Finder assets could not be prepared for the resolved image tree.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("Finder layout folder is absent: {folder:?}")]
	MissingFolder { folder: String },
	#[error("Finder layout target is not a folder: {folder:?}")]
	NotDirectory { folder: String },
	#[error("Finder layout folder is ambiguous under HFS+ comparison: {folder:?}")]
	AmbiguousFolder { folder: String },
	#[error("content conflicts with the reserved Finder store: {path:?}")]
	StoreCollision { path: VirtualPath },
	#[error("Finder metadata would exceed the build's entry limit")]
	EntryBudget,
	#[error("Finder metadata would exceed the build's metadata byte limit")]
	MetadataBudget,
	#[error("cannot reserve Finder planning memory")]
	Reserve(#[from] std::collections::TryReserveError),
	#[error("cannot represent a Finder store path in {folder:?}")]
	StorePath {
		folder: String,
		#[source]
		source: VirtualPathError,
	},
	#[error("cannot generate Finder layout for {folder:?}")]
	Layout {
		folder: String,
		#[source]
		source: dmgez_finder::Error,
	},
	#[error("Finder metadata generation was cancelled")]
	Cancelled,
	#[error("content conflicts with the reserved Finder background directory: {path:?}")]
	BackgroundCollision { path: VirtualPath },
	#[error("Finder background must contain 1 to {max} encoded bytes", max = background::MAX_FILE_BYTES)]
	BackgroundFileSize,
	#[error("Finder background exceeds the admitted working memory or dimensions")]
	BackgroundMemory,
	#[error("animated PNG images are unsupported as Finder backgrounds")]
	BackgroundAnimated,
	#[error("Finder background source failed its identity checks")]
	BackgroundSource(#[source] crate::manifest::Error),
	#[error("could not read the captured Finder background")]
	BackgroundRead(#[source] dmgez_core::source::SourceError),
	#[error("Finder background must be a valid PNG")]
	BackgroundPng(#[source] png::DecodingError),
	#[error("Retina Finder background must be twice the base width and height")]
	BackgroundRetinaDimensions,
	#[error("Retina Finder background PNG representation is unsupported")]
	BackgroundRetinaPngFormat,
	#[error("Finder background TIFF could not be encoded or decoded")]
	BackgroundTiff(#[source] tiff::TiffError),
	#[error(
		"Finder background TIFF must contain compatible 1x and 2x representations with matching logical sizes"
	)]
	BackgroundTiffRepresentation,
	#[error("Finder background alias names a different path than the captured image")]
	BackgroundTargetMismatch,
	#[cfg(feature = "volume-icon")]
	#[error("content conflicts with the reserved volume icon: {path:?}")]
	VolumeIconCollision { path: VirtualPath },
	#[cfg(feature = "volume-icon")]
	#[error("volume icon validation exceeds the available working memory")]
	VolumeIconMemory,
	#[cfg(feature = "volume-icon")]
	#[error("volume icon source failed its identity checks")]
	VolumeIconSource(#[from] crate::manifest::Error),
	#[cfg(feature = "volume-icon")]
	#[error("could not read the captured volume icon")]
	VolumeIconRead(#[from] dmgez_core::source::SourceError),
	#[cfg(feature = "volume-icon")]
	#[error("could not prepare the Finder volume icon")]
	VolumeIcon(#[from] dmgez_finder::volume_icon::Error),
}

/// Generates Finder stores without opening host paths or consulting host identities.
///
/// The root, configured folders, and folders with positioned children receive
/// stores in stable folder-path order. Root style defaults apply to each
/// nested store. Position maps remain scoped to their own folders.
///
/// # Errors
///
/// Fails when a folder, position, or generated path is invalid or ambiguous,
/// budgets cannot accommodate the generated metadata, or the plan is cancelled.
/// Image backgrounds require filesystem identity binding through build preparation.
/// Supply the manifest and execution profile for the same project.
pub fn generate(
	project: &Project,
	manifest: &Manifest,
	profile: &ExecutionProfile,
) -> Result<Vec<GeneratedAsset>, Error> {
	if profile.finder() == FinderPlan::NoStore {
		return Ok(Vec::new());
	}
	let mut budget = AssetBudget::new(manifest, profile)?;
	Ok(plan_stores(project, manifest, profile, &mut budget, false)?
		.assets
		.ready)
}

#[derive(Debug, Default)]
pub(crate) struct PreparedAssets {
	pub(crate) ready: Vec<GeneratedAsset>,
	pub(crate) directories: Vec<VirtualPath>,
	pub(crate) deferred: Vec<DeferredFinderStore>,
	pub(crate) deferred_metadata_bytes: usize,
	#[cfg(feature = "volume-icon")]
	pub(crate) root_finder_info: volume_icon::RootFinderInfo,
}

#[derive(Default)]
struct PlannedStores {
	assets: PreparedAssets,
	backgrounds: Vec<BackgroundSource>,
	deferred: Vec<(PendingFinderStore, usize)>,
}

pub(crate) fn prepare(
	project: &Project,
	manifest: &mut Manifest,
	profile: &ExecutionProfile,
	config_dir: &Path,
	output: Option<&LocalPath>,
	#[cfg(feature = "volume-icon")] volume_icon: Option<volume_icon::IconSources>,
) -> Result<PreparedAssets, Error> {
	let mut budget = AssetBudget::new(manifest, profile)?;
	let PlannedStores {
		mut assets,
		backgrounds,
		deferred,
	} = plan_stores(project, manifest, profile, &mut budget, true)?;
	let sort_files = !backgrounds.is_empty();
	#[cfg(feature = "volume-icon")]
	let sort_files = sort_files || volume_icon.is_some();
	let first_background = assets.ready.len();
	assets.ready.try_reserve_exact(backgrounds.len())?;
	for (index, background) in backgrounds.into_iter().enumerate() {
		assets.ready.push(background.capture(
			index,
			manifest,
			profile,
			config_dir,
			output,
			&mut budget,
		)?);
	}
	assets.deferred.try_reserve_exact(deferred.len())?;
	for (store, index) in deferred {
		let path = &assets.ready[first_background + index].path;
		let (bound, retained) = store.bind(path, &mut budget)?;
		assets.deferred_metadata_bytes = assets
			.deferred_metadata_bytes
			.checked_add(retained)
			.ok_or(Error::MetadataBudget)?;
		assets.deferred.push(bound);
	}
	#[cfg(feature = "volume-icon")]
	if let Some(icon) = volume_icon {
		let (asset, flags) = volume_icon::prepare_asset(icon, manifest, profile, &mut budget)?;
		assets.ready.try_reserve_exact(1)?;
		assets.ready.push(asset);
		assets.root_finder_info = flags;
	}
	if sort_files {
		assets
			.ready
			.sort_unstable_by(|left, right| left.path.cmp(&right.path));
	}
	Ok(assets)
}

fn plan_stores(
	project: &Project,
	manifest: &Manifest,
	profile: &ExecutionProfile,
	budget: &mut AssetBudget,
	allow_deferred: bool,
) -> Result<PlannedStores, Error> {
	if profile.finder() == FinderPlan::NoStore {
		return Ok(PlannedStores::default());
	}
	check_cancelled(profile)?;

	let root_options = project.finder.as_ref().map(|finder| &finder.options);
	let mut folders = layout_folders(project, manifest, profile, budget.entries)?;
	budget.reserve_assets(folders.len())?;
	assign_children(manifest, profile, &mut folders)?;
	let inherited = root_options
		.filter(|_| folders.len() > 1)
		.map(without_positions);
	let mut plan = PlannedStores::default();
	let mut backgrounds_by_source = BTreeMap::new();
	plan.assets.ready.try_reserve_exact(folders.len())?;
	for (path, folder) in folders {
		check_cancelled(profile)?;
		let store = store_path(path)?;
		let options = if path == IMAGE_ROOT {
			root_options.map(Cow::Borrowed)
		} else {
			resolve_options(inherited.as_ref(), folder.options)
		};
		if allow_deferred
			&& let Some(background) = options
				.as_deref()
				.and_then(|settings| settings.background.as_ref())
			&& let Some(image) = plan.add_background(
				background,
				path,
				manifest,
				&mut backgrounds_by_source,
				budget,
			)? {
			let deferred = PendingFinderStore::plan(
				store,
				options.expect("an image was selected"),
				&folder.children,
				budget,
			)?;
			plan.deferred.try_reserve_exact(1)?;
			plan.deferred.push((deferred, image));
			continue;
		}
		budget.charge(store.as_str().len())?;
		let bytes =
			write_store(options.as_deref(), &folder.children).map_err(|source| Error::Layout {
				folder: path.to_owned(),
				source,
			})?;
		budget.charge(bytes.capacity())?;
		plan.assets
			.ready
			.push(GeneratedAsset { path: store, bytes });
	}
	Ok(plan)
}

pub(crate) struct AssetBudget {
	entries: usize,
	metadata_bytes: usize,
}

impl AssetBudget {
	pub(crate) fn new(manifest: &Manifest, profile: &ExecutionProfile) -> Result<Self, Error> {
		let limits = profile.limits();
		let entries = limits
			.max_entries
			.checked_sub(manifest.entries().len())
			.ok_or(Error::EntryBudget)?;
		let used = usize::try_from(manifest.metadata_bytes()).map_err(|_| Error::MetadataBudget)?;
		let metadata_bytes = limits
			.max_metadata_bytes
			.checked_sub(used)
			.ok_or(Error::MetadataBudget)?;
		Ok(Self {
			entries,
			metadata_bytes,
		})
	}

	fn reserve_assets(&mut self, count: usize) -> Result<(), Error> {
		self.entries = self.entries.checked_sub(count).ok_or(Error::EntryBudget)?;
		let bytes = size_of::<GeneratedAsset>()
			.checked_mul(count)
			.ok_or(Error::MetadataBudget)?;
		self.charge(bytes)
	}

	fn charge(&mut self, bytes: usize) -> Result<(), Error> {
		self.metadata_bytes = self
			.metadata_bytes
			.checked_sub(bytes)
			.ok_or(Error::MetadataBudget)?;
		Ok(())
	}
}

#[derive(Default)]
struct Folder<'a> {
	options: Option<&'a FinderOptions>,
	children: Vec<LayoutItem<'a>>,
}

struct RequestedFolder<'a> {
	path: &'a str,
	options: &'a FinderOptions,
	matched: bool,
}

fn layout_folders<'a>(
	project: &'a Project,
	manifest: &'a Manifest,
	profile: &ExecutionProfile,
	entry_budget: usize,
) -> Result<BTreeMap<&'a str, Folder<'a>>, Error> {
	if entry_budget == 0 {
		return Err(Error::EntryBudget);
	}
	let mut folders = BTreeMap::from([(
		IMAGE_ROOT,
		Folder {
			options: project.finder.as_ref().map(|finder| &finder.options),
			children: Vec::new(),
		},
	)]);
	let mut requests = BTreeMap::new();
	if let Some(finder) = &project.finder {
		if finder.folders.len() >= entry_budget {
			return Err(Error::EntryBudget);
		}
		for folder in &finder.folders {
			check_cancelled(profile)?;
			let path = folder.path.as_str();
			let request = RequestedFolder {
				path,
				options: &folder.options,
				matched: false,
			};
			let duplicate = requests
				.insert(path.to_ascii_lowercase(), request)
				.is_some();
			if duplicate || folder.path.is_root() {
				return Err(Error::AmbiguousFolder {
					folder: path.to_owned(),
				});
			}
		}
	}

	for entry in manifest.entries() {
		check_cancelled(profile)?;
		let path = entry.destination.as_str();
		if !requests.is_empty()
			&& let Some(request) = requests.get_mut(&path.to_ascii_lowercase())
		{
			if request.matched {
				return Err(Error::AmbiguousFolder {
					folder: request.path.to_owned(),
				});
			}
			if !matches!(entry.kind, EntryKind::Directory) {
				return Err(Error::NotDirectory {
					folder: request.path.to_owned(),
				});
			}
			request.matched = true;
			folders.entry(path).or_default().options = Some(request.options);
		}
		if entry.finder.position.is_some() {
			let (parent, _) = parent_and_name(&entry.destination);
			folders.entry(parent).or_default();
		}
		if folders.len() > entry_budget {
			return Err(Error::EntryBudget);
		}
	}

	if let Some(missing) = requests.values().find(|request| !request.matched) {
		return Err(Error::MissingFolder {
			folder: missing.path.to_owned(),
		});
	}
	let mut names = BTreeSet::new();
	for path in folders.keys() {
		if !names.insert(path.to_ascii_lowercase()) {
			return Err(Error::AmbiguousFolder {
				folder: (*path).to_owned(),
			});
		}
	}
	Ok(folders)
}

fn assign_children<'a>(
	manifest: &'a Manifest,
	profile: &ExecutionProfile,
	folders: &mut BTreeMap<&'a str, Folder<'a>>,
) -> Result<(), Error> {
	for entry in manifest.entries() {
		check_cancelled(profile)?;
		let (parent, name) = parent_and_name(&entry.destination);
		let Some(folder) = folders.get_mut(parent) else {
			continue;
		};
		if name.eq_ignore_ascii_case(STORE_NAME) {
			return Err(Error::StoreCollision {
				path: entry.destination.clone(),
			});
		}
		let configured_position = folder
			.options
			.and_then(|options| options.icon_view.as_ref())
			.is_some_and(|icons| icons.positions.contains_key(name));
		if configured_position || entry.finder.position.is_some() {
			folder.children.try_reserve(1)?;
			folder.children.push(LayoutItem {
				name,
				position: entry.finder.position,
			});
		}
	}
	Ok(())
}

fn parent_and_name(path: &VirtualPath) -> (&str, &str) {
	let (parent, name) = path
		.as_str()
		.rsplit_once(PATH_SEPARATOR)
		.expect("a virtual path has a root separator");
	(
		if parent.is_empty() {
			IMAGE_ROOT
		} else {
			parent
		},
		name,
	)
}

fn store_path(folder: &str) -> Result<VirtualPath, Error> {
	let parent = if folder == IMAGE_ROOT { "" } else { folder };
	format!("{parent}{PATH_SEPARATOR}{STORE_NAME}")
		.parse()
		.map_err(|source| Error::StorePath {
			folder: folder.to_owned(),
			source,
		})
}

fn check_cancelled(profile: &ExecutionProfile) -> Result<(), Error> {
	if profile.write_options().cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	Ok(())
}
