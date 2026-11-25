//! Bounded native-name queries on an admitted HFS+ or HFSX image volume.

use std::{fmt::Write as _, num::NonZeroU16};

use dmgez_hfsplus_read::browse::{self, VolumeSignature};
use dmgez_inspector::{DiskLimits, volume_queries::MAX_LIST_OFFSET};
use dmgez_udif::{
	Cancellation, Error as ImageError,
	reader::{Image, Limits, ReadAt},
};
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{
		Metadata, NodeHandle, NodeKind, RawName, StorageId, ViewError, VolumeBackend, VolumeView,
	},
};
use thiserror::Error;

use super::{ImageSnapshot, ImageSource};

#[cfg(feature = "actual-preview")]
pub(crate) mod preview;

pub(crate) const PAGE_ENTRIES: u64 = 64;
const SCAN_BATCH_ENTRIES: u16 = 128;
const MAX_DEPTH: usize = 128;
const MAX_FS_READS: u64 = 131_072;
const MAX_FS_READ_BYTES: u64 = 256 << 20;

#[derive(Default)]
pub(crate) struct Browser {
	pub showing: bool,
	pub page: Option<Page>,
	pub details: Option<Details>,
	#[cfg(feature = "actual-preview")]
	pub preview: Option<preview::Presented>,
}

impl Browser {
	pub(crate) fn invalidate(&mut self) {
		self.page = None;
		self.details = None;
		#[cfg(feature = "actual-preview")]
		{
			self.preview = None;
		}
	}

	pub(crate) fn show_page(&mut self, page: Page) {
		self.page = Some(page);
		self.details = None;
		#[cfg(feature = "actual-preview")]
		{
			self.preview = None;
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Location {
	pub volume: VolumeId,
	pub path: Vec<RawName>,
}

impl Location {
	pub(crate) fn root(volume: VolumeId) -> Self {
		Self {
			volume,
			path: Vec::new(),
		}
	}

	pub(crate) fn child(&self, name: &RawName) -> Option<Self> {
		if self.path.len() >= MAX_DEPTH {
			return None;
		}
		let mut child = self.clone();
		child.path.push(name.clone());
		Some(child)
	}

	pub(crate) fn ancestor(&self, depth: usize) -> Self {
		Self {
			volume: self.volume,
			path: self.path[..depth].to_vec(),
		}
	}
}

#[derive(Debug)]
pub(crate) enum Request {
	Page {
		location: Location,
		offset: u64,
	},
	Details {
		folder: Location,
		name: RawName,
	},
	#[cfg(feature = "actual-preview")]
	Preview {
		location: Location,
		max_side: usize,
	},
}

impl Request {
	pub(crate) fn first(location: Location) -> Self {
		Self::Page {
			location,
			offset: 0,
		}
	}
}

#[derive(Debug)]
pub(crate) enum Observation {
	Page(Page),
	Details(Details),
	#[cfg(feature = "actual-preview")]
	Preview(preview::Prepared),
}

#[derive(Debug)]
pub(crate) struct Page {
	pub location: Location,
	pub filesystem: VolumeSignature,
	pub entries: Vec<Entry>,
	pub offset: u64,
	pub next: Option<u64>,
	pub truncated: bool,
}

#[derive(Debug)]
pub(crate) struct Entry {
	pub name: RawName,
	pub kind: NodeKind,
}

#[derive(Debug)]
pub(crate) struct Details {
	pub name: RawName,
	pub native_id: Option<u64>,
	pub metadata: Metadata<StorageId>,
	pub link_target: Option<Vec<u8>>,
}

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error(transparent)]
	Image(#[from] ImageError),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[error(transparent)]
	Filesystem(#[from] browse::Error),
	#[error(transparent)]
	View(#[from] ViewError<browse::Error>),
	#[error("folder nesting exceeds the supported depth")]
	DepthLimit,
	#[error("directory offset exceeds the supported browsing limit")]
	OffsetLimit,
	#[error("stored directory entry no longer resolves by its exact name")]
	EntryChanged,
	#[error("the requested folder contains a non-directory path component")]
	NotDirectory,
	#[error("catalog returned an empty incomplete page")]
	StalledDirectory,
	#[cfg(feature = "actual-preview")]
	#[error(transparent)]
	PreviewCapture(#[from] dmgez_hfsplus_read::preview::CaptureError),
	#[cfg(feature = "actual-preview")]
	#[error(transparent)]
	PreviewRaster(#[from] dmgez_preview::raster::Error),
	#[cfg(feature = "actual-preview")]
	#[error("the folder name encoding cannot be used for this preview")]
	PreviewNameEncoding,
	#[cfg(feature = "actual-preview")]
	#[error("the folder preview exceeds the desktop texture limit")]
	PreviewTextureLimit,
	#[cfg(feature = "actual-preview")]
	#[error("the folder preview has inconsistent RGB data")]
	PreviewTextureData,
}

impl ImageSnapshot {
	pub(crate) fn browse(
		&self,
		request: Request,
		cancellation: &Cancellation,
	) -> Result<Observation, Error> {
		let source = CancellableSource {
			source: &self.source,
			cancellation,
		};
		source.check_unchanged()?;
		let result = browse_image(source, request);
		self.source.check_unchanged()?;
		if cancellation.is_cancelled() {
			return Err(ImageError::Cancelled.into());
		}
		result
	}
}

fn browse_image(source: impl ReadAt, request: Request) -> Result<Observation, Error> {
	let location = match &request {
		Request::Page { location, .. } => location,
		Request::Details { folder, .. } => folder,
		#[cfg(feature = "actual-preview")]
		Request::Preview { location, .. } => location,
	};
	let image = Image::open_release(source, Limits::default())?;
	let mut disk = Disk::discover(image, DiskLimits::default())?;
	let selected = disk.select_volume(Some(location.volume))?;
	let limits = browse::Limits {
		max_read_requests: MAX_FS_READS,
		max_read_bytes: MAX_FS_READ_BYTES,
		..browse::Limits::default()
	};
	match request {
		Request::Page { location, offset } => {
			browse::with_view_signature(&mut disk, selected, limits, |view, filesystem| {
				folder_page(view, location, filesystem, offset)
			})?
		}
		Request::Details { folder, name } => {
			browse::with_view(&mut disk, selected, limits, |view| {
				entry_details(view, &folder, name)
			})?
		}
		#[cfg(feature = "actual-preview")]
		Request::Preview { location, max_side } => {
			preview::render(&mut disk, selected, limits, location, max_side)
		}
	}
}

fn folder<B: VolumeBackend<Error = browse::Error>>(
	view: &mut VolumeView<B>,
	location: &Location,
) -> Result<NodeHandle<B::Id>, Error> {
	if location.path.len() > MAX_DEPTH {
		return Err(Error::DepthLimit);
	}
	let mut node = view.root()?;
	for name in &location.path {
		let entry = view
			.lookup(&node, name.as_lookup())?
			.filter(|entry| entry.name == *name)
			.ok_or(Error::EntryChanged)?;
		if entry.kind != NodeKind::Directory {
			return Err(Error::NotDirectory);
		}
		node = entry.node;
	}
	Ok(node)
}

fn folder_page(
	view: &mut VolumeView<impl VolumeBackend<Error = browse::Error>>,
	location: Location,
	filesystem: VolumeSignature,
	offset: u64,
) -> Result<Observation, Error> {
	if offset > MAX_LIST_OFFSET {
		return Err(Error::OffsetLimit);
	}
	let node = folder(view, &location)?;
	let mut reader = view.read_dir(&node)?;
	let end = offset + PAGE_ENTRIES;
	let mut scanned = 0;
	let mut entries = Vec::new();
	while scanned <= end {
		// Read one extra catalog entry to distinguish a final page with no spare rows.
		let remaining = (end + 1 - scanned).min(u64::from(SCAN_BATCH_ENTRIES)) as u16;
		let page = reader.next_page(NonZeroU16::new(remaining).ok_or(Error::StalledDirectory)?)?;
		if page.entries.is_empty() && !page.complete {
			return Err(Error::StalledDirectory);
		}
		for entry in page.entries {
			if (offset..end).contains(&scanned) {
				entries.push(Entry {
					name: entry.name,
					kind: entry.kind,
				});
			}
			scanned += 1;
		}
		if page.complete {
			break;
		}
	}
	let more = scanned > end;
	Ok(Observation::Page(Page {
		location,
		filesystem,
		entries,
		offset,
		next: (more && end <= MAX_LIST_OFFSET).then_some(end),
		truncated: more && end > MAX_LIST_OFFSET,
	}))
}

fn entry_details(
	view: &mut VolumeView<impl VolumeBackend<Error = browse::Error>>,
	location: &Location,
	name: RawName,
) -> Result<Observation, Error> {
	let parent = folder(view, location)?;
	let entry = view
		.lookup(&parent, name.as_lookup())?
		.filter(|entry| entry.name == name)
		.ok_or(Error::EntryChanged)?;
	let metadata = view.stat(&entry.node)?;
	let native_id = view.numeric_id(&entry.node)?;
	let link_target = if metadata.kind == NodeKind::Symlink {
		Some(view.read_link(&entry.node)?)
	} else {
		None
	};
	Ok(Observation::Details(Details {
		name,
		native_id,
		metadata,
		link_target,
	}))
}

pub(crate) fn label(name: &RawName) -> String {
	match name {
		RawName::Utf8(name) => name.escape_debug().to_string(),
		RawName::Utf16(name) => {
			let mut label = String::new();
			for unit in char::decode_utf16(name.iter().copied()) {
				match unit {
					Ok(character) => label.extend(character.escape_debug()),
					Err(error) => {
						let _ = write!(label, "\\u{{{:x}}}", error.unpaired_surrogate());
					}
				}
			}
			label
		}
		_ => "Unknown name".to_owned(),
	}
}

struct CancellableSource<'a> {
	source: &'a ImageSource,
	cancellation: &'a Cancellation,
}

impl ReadAt for CancellableSource<'_> {
	fn len(&self) -> u64 {
		self.source.len()
	}

	fn check_unchanged(&self) -> Result<(), ImageError> {
		if self.cancellation.is_cancelled() {
			return Err(ImageError::Cancelled);
		}
		self.source.check_unchanged()
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), ImageError> {
		if self.cancellation.is_cancelled() {
			return Err(ImageError::Cancelled);
		}
		self.source.read_exact_at(offset, output)
	}
}

#[cfg(test)]
mod tests;
