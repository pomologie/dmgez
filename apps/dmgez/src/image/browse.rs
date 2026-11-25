//! Bounded native-name queries on admitted image volumes.

use std::fmt::Write as _;

#[cfg(feature = "hfsplus-read")]
use dmgez_hfsplus::browse::{self, VolumeSignature};
#[cfg(feature = "hfsplus-read")]
use dmgez_inspector::DiskLimits;
use dmgez_inspector::volume_queries::{SearchError, StorageMetadata};
#[cfg(feature = "actual-preview")]
use dmgez_preview::ItemVisibility;
use dmgez_runtime::disk::{Encoding, with_logical_disk};
use dmgez_udif::{
	Cancellation, Error as ImageError,
	reader::{Limits, ReadAt},
};
#[cfg(feature = "hfsplus-read")]
use dmgez_vfs::{Disk, LogicalDisk};
use dmgez_vfs::{
	VolumeId,
	volume::{Metadata, NodeKind, RawName, StorageId, ViewError},
};
use thiserror::Error;

use super::{ImageSnapshot, ImageSource};

#[cfg(feature = "apfs-read")]
pub(crate) mod apfs;
pub(crate) mod content;
pub(crate) mod history;
#[cfg(feature = "actual-preview")]
pub(crate) mod preview;
mod query;
pub(crate) mod search;

pub(crate) const PAGE_ENTRIES: u64 = 64;
const MAX_DEPTH: usize = 128;
#[cfg(feature = "hfsplus-read")]
const MAX_FS_READS: u64 = 131_072;
#[cfg(feature = "hfsplus-read")]
const MAX_FS_READ_BYTES: u64 = 256 << 20;

#[derive(Default)]
pub(crate) struct Browser {
	pub showing: bool,
	pub page: Option<Page>,
	pub details: Option<Details>,
	pub history: history::History,
	pub search: search::State,
	#[cfg(feature = "apfs-read")]
	pub apfs: Option<apfs::Catalog>,
	#[cfg(feature = "actual-preview")]
	pub preview: Option<preview::Presented>,
}

impl Browser {
	pub(crate) fn invalidate(&mut self) {
		self.page = None;
		self.details = None;
		self.history = history::History::default();
		self.search.clear();
		#[cfg(feature = "apfs-read")]
		{
			self.apfs = None;
		}
		#[cfg(feature = "actual-preview")]
		{
			self.preview = None;
		}
	}

	pub(crate) fn show_page(&mut self, page: Page, direction: Option<history::Direction>) -> bool {
		if !self.history.arrived(self.page.as_ref(), &page, direction) {
			return false;
		}
		if self
			.page
			.as_ref()
			.is_none_or(|old| old.location != page.location)
		{
			self.search.clear();
		}
		self.page = Some(page);
		self.details = None;
		#[cfg(feature = "actual-preview")]
		{
			self.preview = None;
		}
		true
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Location {
	pub volume: VolumeId,
	pub path: Vec<RawName>,
	#[cfg(feature = "apfs-read")]
	pub apfs: Option<apfs::Selection>,
}

impl Location {
	pub(crate) fn root(volume: VolumeId) -> Self {
		Self {
			volume,
			path: Vec::new(),
			#[cfg(feature = "apfs-read")]
			apfs: None,
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
			#[cfg(feature = "apfs-read")]
			apfs: self.apfs,
		}
	}
}

#[derive(Debug)]
pub(crate) enum Request {
	#[cfg(feature = "apfs-read")]
	ApfsCatalog(VolumeId),
	Page {
		location: Location,
		offset: u64,
	},
	Details {
		folder: Location,
		name: RawName,
	},
	Text {
		folder: Location,
		name: RawName,
	},
	Search {
		location: Location,
		query: String,
	},
	#[cfg(feature = "actual-preview")]
	Preview {
		location: Location,
		max_side: usize,
		visibility: ItemVisibility,
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
	#[cfg(feature = "apfs-read")]
	ApfsCatalog(apfs::Catalog),
	Page(Page),
	Details(Details),
	Search(search::Report),
	#[cfg(feature = "actual-preview")]
	Preview(preview::Prepared),
}

#[derive(Debug)]
pub(crate) struct Page {
	pub location: Location,
	pub filesystem: Filesystem,
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
	pub folder: Location,
	pub name: RawName,
	pub native_id: Option<u64>,
	pub metadata: Metadata<StorageId>,
	pub link_target: Option<Vec<u8>>,
	pub storage: Box<Storage>,
	pub text: Option<content::Preview>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Filesystem {
	#[cfg(feature = "hfsplus-read")]
	HfsPlus,
	#[cfg(feature = "hfsplus-read")]
	HfsX,
	#[cfg(feature = "apfs-read")]
	Apfs,
}

impl Filesystem {
	pub(crate) const fn label(self) -> &'static str {
		match self {
			#[cfg(feature = "hfsplus-read")]
			Self::HfsPlus => "HFS+",
			#[cfg(feature = "hfsplus-read")]
			Self::HfsX => "HFSX",
			#[cfg(feature = "apfs-read")]
			Self::Apfs => "APFS",
		}
	}

	#[cfg(feature = "actual-preview")]
	pub(crate) const fn supports_finder_preview(self) -> bool {
		matches!(self, Self::HfsPlus | Self::HfsX)
	}
}

#[derive(Debug)]
pub(crate) enum Storage {
	#[cfg(feature = "hfsplus-read")]
	Hfs(StorageMetadata<browse::Error>),
	#[cfg(feature = "apfs-read")]
	Apfs(StorageMetadata<apfs::BackendError>),
}

#[cfg(feature = "hfsplus-read")]
impl From<StorageMetadata<browse::Error>> for Storage {
	fn from(storage: StorageMetadata<browse::Error>) -> Self {
		Self::Hfs(storage)
	}
}

#[cfg(feature = "apfs-read")]
impl From<StorageMetadata<apfs::BackendError>> for Storage {
	fn from(storage: StorageMetadata<apfs::BackendError>) -> Self {
		Self::Apfs(storage)
	}
}

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error(transparent)]
	Image(#[from] ImageError),
	#[error(transparent)]
	Disk(#[from] dmgez_vfs::Error),
	#[cfg(feature = "hfsplus-read")]
	#[error(transparent)]
	Filesystem(#[from] browse::Error),
	#[cfg(feature = "hfsplus-read")]
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
	#[cfg(feature = "hfsplus-read")]
	#[error(transparent)]
	Search(#[from] SearchError<browse::Error>),
	#[cfg(feature = "apfs-read")]
	#[error(transparent)]
	ApfsWindow(#[from] dmgez_runtime::volume::Error),
	#[cfg(feature = "apfs-read")]
	#[error(transparent)]
	ApfsOpen(#[from] dmgez_apfs::OpenError),
	#[cfg(feature = "apfs-read")]
	#[error(transparent)]
	ApfsView(#[from] ViewError<apfs::BackendError>),
	#[cfg(feature = "apfs-read")]
	#[error(transparent)]
	ApfsSearch(#[from] SearchError<apfs::BackendError>),
	#[cfg(feature = "apfs-read")]
	#[error("the APFS volume at this checkpoint no longer matches the selected identity")]
	ApfsVolumeChanged,
	#[cfg(any(feature = "apfs-read", feature = "actual-preview"))]
	#[error("the request requires a supported and selected filesystem volume")]
	UnsupportedVolumeRequest,
	#[cfg(feature = "actual-preview")]
	#[error(transparent)]
	Preview(#[from] dmgez_runtime::preview::actual::RenderError),
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
	with_logical_disk(source, Encoding::Udif(Limits::default()), |logical| {
		let location = match &request {
			Request::Page { location, .. }
			| Request::Search { location, .. }
			| Request::Details {
				folder: location, ..
			}
			| Request::Text {
				folder: location, ..
			} => location,
			#[cfg(feature = "actual-preview")]
			Request::Preview { location, .. } => location,
			#[cfg(feature = "apfs-read")]
			Request::ApfsCatalog(window) => return apfs::discover(logical, *window),
		};
		let volume = location.volume;
		#[cfg(feature = "apfs-read")]
		if let Some(selection) = location.apfs {
			return apfs::browse(logical, volume, selection, request);
		}
		#[cfg(feature = "hfsplus-read")]
		{
			browse_hfs(logical, volume, request)
		}
		#[cfg(not(feature = "hfsplus-read"))]
		{
			Err(Error::UnsupportedVolumeRequest)
		}
	})
}

#[cfg(feature = "hfsplus-read")]
fn browse_hfs(
	logical: &mut dyn LogicalDisk,
	volume: VolumeId,
	request: Request,
) -> Result<Observation, Error> {
	let mut disk = Disk::discover(logical, DiskLimits::default())?;
	let selected = disk.select_volume(Some(volume))?;
	let limits = browse::Limits {
		max_read_requests: MAX_FS_READS,
		max_read_bytes: MAX_FS_READ_BYTES,
		..browse::Limits::default()
	};
	#[cfg(feature = "actual-preview")]
	if let Request::Preview {
		location,
		max_side,
		visibility,
	} = request
	{
		return preview::render(&mut disk, selected, limits, location, max_side, visibility);
	}
	browse::with_view_signature(&mut disk, selected, limits, |view, filesystem| {
		let filesystem = match filesystem {
			VolumeSignature::HfsPlus => Filesystem::HfsPlus,
			VolumeSignature::HfsX => Filesystem::HfsX,
		};
		query::query_volume(view, filesystem, request, search::hfs)
	})?
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
mod test_support;
#[cfg(all(test, feature = "hfsplus-read"))]
mod tests;
