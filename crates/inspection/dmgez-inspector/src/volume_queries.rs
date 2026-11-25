//! Bounded queries over a read-only volume view.
//!
//! Paths are absolute and use native lookup for each UTF-8 component. Visible
//! directory entries retain their stored names. Commands never follow links.

mod error;
mod path;
mod report;
mod search;
mod storage;
mod tree;

pub use error::{Action, Error, InvalidPath};
pub use report::{EntryReport, Kind, ListReport, Name, StatReport, TreeEntryReport, TreeReport};
pub use search::{SearchEntry, SearchError, SearchExtent, SearchReport, search_children};
pub use storage::{ForkSizes, StorageMetadata, inspect_storage};
pub use tree::{DEFAULT_TREE_DEPTH, DEFAULT_TREE_LIMIT, MAX_TREE_DEPTH, MAX_TREE_LIMIT, tree};

use std::{io::Write, num::NonZeroU16};

use dmgez_vfs::volume::{
	DirectoryPage, DirectoryReader, DirectoryView, Fork, ForkKind, NodeHandle, NodeKind,
	VolumeBackend, VolumeView,
};

use crate::{FilesystemObjectId, ReportOperation, SCHEMA_VERSION, SelectedWindowId};

use path::resolve;

/// Default number of entries displayed for one directory page.
pub const DEFAULT_LIST_LIMIT: u16 = 128;
/// Maximum number of entries returned to a caller per list request.
pub const MAX_LIST_LIMIT: u16 = 1024;
/// Maximum zero-based entry offset that a list request can scan past.
pub const MAX_LIST_OFFSET: u64 = 100_000;
/// Maximum number of bytes streamed by a single fork query.
pub const MAX_FORK_BYTES: u64 = 4 * (1 << 30);

const SCAN_PAGE_ENTRIES: NonZeroU16 = NonZeroU16::new(128).unwrap();
const FORK_CHUNK_BYTES: usize = 64 * 1024;

/// Resolves an absolute volume path to a handle bound to this view.
///
/// The terminal component can be a link. Intermediate components must be
/// directories, so resolving the path never follows a link target.
///
/// # Errors
/// Rejects invalid or missing paths and propagates lookup failures.
pub fn resolve_node<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
) -> Result<NodeHandle<B::Id>, Error<B::Error>> {
	Ok(resolve(view, path)?.node)
}

/// Lists a page of visible children in native directory order.
///
/// `offset` is zero-based. The query looks ahead by one entry so an exact-fit
/// final page has no continuation. Names are retained in their stored encoding.
///
/// # Errors
/// Rejects invalid paths or query limits and propagates directory errors.
pub fn list<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
	offset: u64,
	limit: u16,
) -> Result<ListReport, Error<B::Error>> {
	if offset > MAX_LIST_OFFSET {
		return Err(Error::ListOffset {
			maximum: MAX_LIST_OFFSET,
		});
	}
	if limit == 0 || limit > MAX_LIST_LIMIT {
		return Err(Error::ListLimit {
			maximum: MAX_LIST_LIMIT,
		});
	}
	let volume = SelectedWindowId::from_volume(view.partition()).ok_or(Error::UnsupportedVolume)?;
	let selected = resolve(view, path)?;
	let mut directory = view
		.read_dir(&selected.node)
		.map_err(|error| Error::view(Action::Directory, error))?;
	let mut entries = Vec::new();
	entries.try_reserve_exact(usize::from(limit))?;
	let end = offset + u64::from(limit);
	let scan_end = end + 1;
	let mut scanned = 0;
	while scanned < scan_end {
		let page = next_page(&mut directory, scan_end - scanned)?;
		for entry in page.entries {
			let ordinal = scanned;
			scanned += 1;
			if (offset..end).contains(&ordinal) {
				let name = Name::from_native(entry.name).ok_or(Error::UnsupportedName)?;
				entries.push(EntryReport {
					name,
					kind: entry.kind.into(),
				});
			}
		}
		if page.complete {
			break;
		}
	}
	let more = scanned > end;
	Ok(ListReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Ls,
		volume,
		path: path.into(),
		entries,
		next_offset: (more && end <= MAX_LIST_OFFSET).then_some(end),
		truncated: more && end > MAX_LIST_OFFSET,
	})
}

/// Returns metadata for a visible entry and the stored target for a symlink.
/// The query does not open file forks or dereference links.
///
/// # Errors
/// Rejects invalid or missing paths and propagates metadata or link-read errors.
pub fn stat<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
) -> Result<StatReport, Error<B::Error>> {
	let volume = SelectedWindowId::from_volume(view.partition()).ok_or(Error::UnsupportedVolume)?;
	let selected = resolve(view, path)?;
	let metadata = view
		.stat(&selected.node)
		.map_err(|error| Error::view(Action::Metadata, error))?;
	let native_id = view
		.numeric_id(&selected.node)
		.map_err(|error| Error::view(Action::Identity, error))?
		.map(FilesystemObjectId);
	let link_target = if metadata.kind == NodeKind::Symlink {
		Some(
			view.read_link(&selected.node)
				.map_err(|error| Error::view(Action::Link, error))?,
		)
	} else {
		None
	};
	let hard_link_count = metadata.hard_link.as_ref().map(|link| link.count.get());
	let hard_link_storage_id = metadata
		.hard_link
		.map(|link| FilesystemObjectId(link.storage.native_id()));
	Ok(StatReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Stat,
		volume,
		path: path.into(),
		name: selected.name,
		kind: metadata.kind.into(),
		mode: metadata.mode,
		uid: metadata.uid,
		gid: metadata.gid,
		native_id,
		hard_link_count,
		hard_link_storage_id,
		link_target,
	})
}

/// Streams exactly the selected fork into the caller's writer in bounded chunks.
///
/// # Errors
/// Rejects invalid paths, non-regular files, absent forks, and forks above the
/// byte ceiling. If a later read or write fails, an already written prefix can
/// remain visible through the writer.
pub fn cat<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
	fork: ForkKind,
	output: &mut impl Write,
) -> Result<u64, Error<B::Error>> {
	let selected = resolve(view, path)?;
	view.with_fork(&selected.node, fork, |reader| copy_fork(reader, output))
		.map_err(|error| Error::view(Action::OpenFork, error))?
		.ok_or(Error::ForkAbsent { kind: fork })?
}

fn copy_fork<E: std::error::Error + 'static>(
	reader: &mut Fork<'_, E>,
	output: &mut impl Write,
) -> Result<u64, Error<E>> {
	let length = reader.len();
	if length > MAX_FORK_BYTES {
		return Err(Error::ForkLimit {
			length,
			maximum: MAX_FORK_BYTES,
		});
	}
	let chunk = length.min(FORK_CHUNK_BYTES as u64) as usize;
	let mut buffer = Vec::new();
	buffer.try_reserve_exact(chunk)?;
	buffer.resize(chunk, 0);
	let mut offset = 0;
	while offset < length {
		let count = (length - offset).min(chunk as u64) as usize;
		let bytes = &mut buffer[..count];
		reader
			.read_exact_at(offset, bytes)
			.map_err(|error| Error::view(Action::ReadFork { offset }, error))?;
		output.write_all(bytes).map_err(Error::Output)?;
		offset += count as u64;
	}
	Ok(offset)
}

fn next_page<D: DirectoryReader>(
	directory: &mut DirectoryView<D>,
	remaining: u64,
) -> Result<DirectoryPage<NodeHandle<D::Id>>, Error<D::Error>> {
	let allowance = remaining.min(u64::from(SCAN_PAGE_ENTRIES.get())) as u16;
	let limit = NonZeroU16::new(allowance).unwrap_or(NonZeroU16::MIN);
	let page = directory
		.next_page(limit)
		.map_err(|error| Error::view(Action::DirectoryPage, error))?;
	if page.entries.is_empty() && !page.complete {
		return Err(Error::StalledDirectory);
	}
	Ok(page)
}
