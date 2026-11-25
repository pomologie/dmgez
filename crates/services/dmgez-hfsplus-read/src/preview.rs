//! Bounded Finder evidence from a selected HFS+ or HFSX data window.

mod background;

use std::{collections::TryReserveError, num::NonZeroU16};

use dmgez_finder::{
	STORE_NAME,
	finder_info::DisplayFlags,
	inspection::{self, ReadError, ReadLimits, Report},
};
pub use dmgez_preview::actual::Entry;
use dmgez_preview::{
	ItemVisibility,
	actual::{self, BackgroundAsset, EntryKind},
	raster::{self, ActualPreview},
};
use dmgez_vfs::{
	Disk, LogicalDisk, VolumeId,
	volume::{
		FinderInfo, ForkKind, LookupName, NodeHandle, NodeKind, RawName, ViewEntry, ViewError,
	},
};
use hfsplus_format::volume::CatalogId;
use thiserror::Error;

use crate::browse::{self, HfsView, Limits};

const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 512;
const MAX_PREVIEW_READS: u64 = 1 << 16;
const MAX_PREVIEW_READ_BYTES: u64 = 64 << 20;
const PAGE_SIZE: NonZeroU16 = NonZeroU16::new(64).unwrap();
const DOT: u16 = b'.' as u16;

/// Evidence status for the selected folder's exact `.DS_Store` entry.
#[derive(Debug)]
pub enum Store {
	Absent,
	Unavailable(CaptureError),
	Unsupported(ReadError),
	Malformed(ReadError),
	Observed(Report),
}

/// State of the image-background reference in the observed store.
#[derive(Debug)]
pub enum Background {
	Absent,
	Unsupported,
	InvalidReference,
	Unavailable(CaptureError),
	/// Exact bytes of the admitted ordinary in-volume file. Image decoding is separate.
	Encoded(Vec<u8>),
}

/// Direct on-image folder contents and any available presentation evidence.
#[derive(Debug)]
#[non_exhaustive]
pub struct Snapshot {
	pub entries: Vec<Entry>,
	pub store: Store,
	pub background: Background,
}

impl Snapshot {
	/// Renders this observed folder into an actual-image preview.
	///
	/// # Errors
	/// Reports inadmissible geometry or exceeded raster budgets.
	pub fn render(self, visibility: ItemVisibility) -> Result<ActualPreview, raster::Error> {
		let store = match &self.store {
			Store::Absent => actual::Store::Absent,
			Store::Unavailable(_) => actual::Store::Unavailable,
			Store::Unsupported(_) => actual::Store::Unsupported,
			Store::Malformed(_) => actual::Store::Malformed,
			Store::Observed(report) => actual::Store::Observed(report),
		};
		let background = match &self.background {
			Background::Encoded(bytes) => BackgroundAsset::Encoded(bytes),
			_ => BackgroundAsset::Unavailable,
		};
		ActualPreview::from_image(&self.entries, store, background, visibility)
	}
}

/// Errors while reading bounded HFS+ presentation evidence.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CaptureError {
	#[error(transparent)]
	Volume(#[from] browse::Error),
	#[error(transparent)]
	View(#[from] ViewError<browse::Error>),
	#[error("folder path exceeds the preview depth limit of {MAX_DEPTH}")]
	Depth,
	#[error("folder path does not identify an exact stored directory")]
	Folder,
	#[error("folder exceeds the preview entry limit of {MAX_ENTRIES}")]
	Entries,
	#[error("catalog did not advance an incomplete directory page")]
	Incomplete,
	#[error("catalog returned a name outside the HFS+ UTF-16 profile")]
	Name,
	#[error("Finder display flags are unavailable")]
	FinderInfo,
	#[error("Finder store must be an ordinary, unshared regular file")]
	StoreEntry,
	#[error("Finder store cannot be inspected within the preview resources")]
	StoreLimit(#[source] ReadError),
	#[error("preview fork exceeds its {limit}-byte limit")]
	ForkLimit { limit: usize },
	#[error("preview data fork is unavailable")]
	MissingFork,
	#[error("could not allocate bounded preview input")]
	Allocation(#[from] TryReserveError),
}

/// Reads one folder in the selected disk window. `components` are exact stored
/// UTF-16 names relative to the volume root. All catalog, metadata and optional
/// asset reads use the same bounded volume session. No host path or link is
/// followed. Presentation evidence failures are retained in the snapshot.
///
/// # Errors
/// Reports unreadable volume/directory data, unrecognized directory components,
/// entry or read budget exhaustion, and unavailable entry display flags.
pub fn capture_disk<S: LogicalDisk>(
	disk: &mut Disk<S>,
	selected: VolumeId,
	mut limits: Limits,
	components: &[Vec<u16>],
) -> Result<Snapshot, CaptureError> {
	if components.len() > MAX_DEPTH {
		return Err(CaptureError::Depth);
	}
	limits.max_read_requests = limits.max_read_requests.min(MAX_PREVIEW_READS);
	limits.max_read_bytes = limits.max_read_bytes.min(MAX_PREVIEW_READ_BYTES);
	browse::with_catalog(disk, selected, limits, |volume, catalog| {
		let mut view = browse::view_for_catalog(selected, volume, catalog, limits);
		let directory = open_folder(&mut view, components)?;
		let entries = capture_entries(&mut view, &directory)?;
		let store = inspect_store(&mut view, &directory);
		let background = match &store {
			Store::Observed(report) => {
				background::capture(&mut view, volume, limits.catalog, report)
			}
			Store::Absent => Background::Absent,
			_ => Background::Unsupported,
		};
		Ok(Snapshot {
			entries,
			store,
			background,
		})
	})?
}

fn open_folder<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	components: &[Vec<u16>],
) -> Result<NodeHandle<CatalogId>, CaptureError> {
	let mut directory = view.root()?;
	for component in components {
		let entry = exact_child(view, &directory, component)?
			.filter(|entry| entry.kind == NodeKind::Directory)
			.ok_or(CaptureError::Folder)?;
		directory = entry.node;
	}
	Ok(directory)
}

fn exact_child<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	parent: &NodeHandle<CatalogId>,
	name: &[u16],
) -> Result<Option<ViewEntry<CatalogId>>, CaptureError> {
	Ok(view
		.lookup(parent, LookupName::StoredUtf16(name))?
		.filter(|entry| matches!(&entry.name, RawName::Utf16(stored) if stored == name)))
}

fn capture_entries<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	directory: &NodeHandle<CatalogId>,
) -> Result<Vec<Entry>, CaptureError> {
	let entries = {
		let mut reader = view.read_dir(directory)?;
		let mut entries = Vec::new();
		loop {
			let page = reader.next_page(PAGE_SIZE)?;
			if entries.len() + page.entries.len() > MAX_ENTRIES {
				return Err(CaptureError::Entries);
			}
			if page.entries.is_empty() && !page.complete {
				return Err(CaptureError::Incomplete);
			}
			entries.try_reserve(page.entries.len())?;
			entries.extend(page.entries);
			if page.complete {
				break entries;
			}
		}
	};
	let mut observed = Vec::new();
	observed.try_reserve_exact(entries.len())?;
	for entry in entries {
		let RawName::Utf16(name) = entry.name else {
			return Err(CaptureError::Name);
		};
		let flags = match view.finder_info(&entry.node)? {
			FinderInfo::Absent => DisplayFlags::default(),
			FinderInfo::Present(info) => DisplayFlags::read(&info),
			_ => return Err(CaptureError::FinderInfo),
		};
		let kind = match entry.kind {
			NodeKind::Directory => EntryKind::Directory,
			NodeKind::RegularFile => EntryKind::File,
			NodeKind::Symlink => EntryKind::Symlink,
			_ => EntryKind::Other,
		};
		observed.push(Entry {
			hidden: name.starts_with(&[DOT]) || flags.hidden.unwrap_or_default(),
			hide_extension: flags.hide_extension.unwrap_or_default(),
			name,
			kind,
		});
	}
	Ok(observed)
}

fn inspect_store<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	directory: &NodeHandle<CatalogId>,
) -> Store {
	let limits = ReadLimits::default();
	let bytes = match read_store(view, directory, limits.store_bytes) {
		Ok(Some(bytes)) => bytes,
		Ok(None) => return Store::Absent,
		Err(error) => return Store::Unavailable(error),
	};
	match inspection::inspect(&bytes, limits) {
		Ok(report) => Store::Observed(report),
		Err(
			error @ (ReadError::UnsupportedTree
			| ReadError::UnsupportedType(_)
			| ReadError::PlistProfile),
		) => Store::Unsupported(error),
		Err(error @ (ReadError::Limit(_) | ReadError::Reserve(_))) => {
			Store::Unavailable(CaptureError::StoreLimit(error))
		}
		Err(error) => Store::Malformed(error),
	}
}

fn read_store<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	directory: &NodeHandle<CatalogId>,
	limit: usize,
) -> Result<Option<Vec<u8>>, CaptureError> {
	let name: Vec<_> = STORE_NAME.encode_utf16().collect();
	let Some(store) = exact_child(view, directory, &name)? else {
		return Ok(None);
	};
	if store.kind != NodeKind::RegularFile || view.stat(&store.node)?.hard_link.is_some() {
		return Err(CaptureError::StoreEntry);
	}
	read_fork(view, &store.node, limit).map(Some)
}

fn read_fork<S: LogicalDisk>(
	view: &mut HfsView<'_, '_, S>,
	node: &NodeHandle<CatalogId>,
	limit: usize,
) -> Result<Vec<u8>, CaptureError> {
	view.with_fork(node, ForkKind::Data, |fork| {
		let length = usize::try_from(fork.len())
			.ok()
			.filter(|length| *length <= limit)
			.ok_or(CaptureError::ForkLimit { limit })?;
		let mut bytes = Vec::new();
		bytes.try_reserve_exact(length)?;
		bytes.resize(length, 0);
		fork.read_exact_at(0, &mut bytes)?;
		Ok(bytes)
	})?
	.ok_or(CaptureError::MissingFork)?
}
