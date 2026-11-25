use std::num::{NonZeroU16, NonZeroU32};

use dmgez_inspector::volume_queries::{MAX_LIST_OFFSET, StorageMetadata, inspect_storage};
use dmgez_udif::Cancellation;
use dmgez_vfs::volume::{NodeHandle, NodeKind, RawName, ViewError, VolumeBackend, VolumeView};

use super::{
	Details, Entry, Error, Filesystem, Location, MAX_DEPTH, Observation, PAGE_ENTRIES, Page,
	Request, Storage, content,
};

const SCAN_BATCH_ENTRIES: u16 = 128;
const MAX_ENTRY_ATTRIBUTES: NonZeroU32 = NonZeroU32::new(64).unwrap();

enum FileQuery<'a> {
	MetadataOnly,
	Text,
	Png(&'a Cancellation),
}

pub(super) fn query_volume<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	filesystem: Filesystem,
	request: Request,
	search: impl FnOnce(&mut VolumeView<B>, Location, String) -> Result<Observation, Error>,
	cancellation: &Cancellation,
) -> Result<Observation, Error>
where
	Error: From<ViewError<B::Error>>,
	Storage: From<StorageMetadata<B::Error>>,
{
	match request {
		Request::Page { location, offset } => folder_page(view, location, filesystem, offset),
		Request::Details { folder, name } => {
			entry_details(view, folder, name, FileQuery::MetadataOnly)
		}
		Request::Text { folder, name } => entry_details(view, folder, name, FileQuery::Text),
		Request::Png { folder, name } => {
			entry_details(view, folder, name, FileQuery::Png(cancellation))
		}
		Request::Search { location, query } => search(view, location, query),
		#[cfg(feature = "actual-preview")]
		Request::Preview { .. } => Err(Error::UnsupportedVolumeRequest),
		#[cfg(feature = "apfs-read")]
		Request::ApfsCatalog(_) => Err(Error::UnsupportedVolumeRequest),
	}
}

pub(super) fn folder<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	location: &Location,
) -> Result<NodeHandle<B::Id>, Error>
where
	Error: From<ViewError<B::Error>>,
{
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

fn folder_page<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	location: Location,
	filesystem: Filesystem,
	offset: u64,
) -> Result<Observation, Error>
where
	Error: From<ViewError<B::Error>>,
	Storage: From<StorageMetadata<B::Error>>,
{
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

fn entry_details<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	location: Location,
	name: RawName,
	request: FileQuery<'_>,
) -> Result<Observation, Error>
where
	Error: From<ViewError<B::Error>>,
	Storage: From<StorageMetadata<B::Error>>,
{
	let parent = folder(view, &location)?;
	let entry = view
		.lookup(&parent, name.as_lookup())?
		.filter(|entry| entry.name == name)
		.ok_or(Error::EntryChanged)?;
	let metadata = view.stat(&entry.node)?;
	let kind = metadata.kind;
	let native_id = view.numeric_id(&entry.node)?;
	let link_target = if kind == NodeKind::Symlink {
		Some(view.read_link(&entry.node)?)
	} else {
		None
	};
	let storage = Box::new(inspect_storage(view, &entry, MAX_ENTRY_ATTRIBUTES).into());
	let mut details = Details {
		folder: location,
		name,
		native_id,
		metadata,
		link_target,
		storage,
		text: None,
		png: None,
	};
	match request {
		FileQuery::MetadataOnly => {}
		FileQuery::Text => details.text = Some(content::read(view, &entry.node, kind)?),
		FileQuery::Png(cancellation) => {
			let image = content::png::read(view, &entry.node, kind, cancellation)?;
			return Ok(Observation::Png(Box::new(content::png::Prepared {
				details,
				image,
			})));
		}
	}
	Ok(Observation::Details(details))
}
