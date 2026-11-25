use std::{
	collections::{BTreeMap, TryReserveError, btree_map::Entry as MapEntry},
	error::Error,
	num::NonZeroUsize,
	rc::Rc,
};

use dmgez_config::{VirtualPath, VirtualPathError};
use dmgez_finder::{STORE_NAME, edit::StoreEditError};
use hfsplus_builder::volume::{EntryMetadata, MAX_SYMLINK_TARGET_BYTES, VolumeError};
use hfsplus_core::{
	ByteOffset, ReadAt,
	names::{MAX_NAME_UNITS, NameError, NormalizedName, normalize, normalize_utf16},
};
use hfsplus_format::{
	catalog::Metadata,
	volume::{CatalogId, Fork},
};
use hfsplus_reader::{
	Volume,
	attributes::AttributeReadError,
	catalog::{Catalog, CatalogError, Entry, EntryKind, HardLinkError},
	check::{CheckError, CheckLimits, StructuralReport},
	overflow::{ForkKind, OverflowError},
};
use thiserror::Error;

use super::{Attributes, File, Kind, Node, Payload, Snapshot};

mod attributes;
mod finder;
mod metadata;

use attributes::AttributeInventory;
use metadata::{BSD_DIRECTORY, BSD_REGULAR_FILE, BSD_SYMLINK, BSD_TYPE_MASK};

pub(super) const MAX_ENTRIES: usize = 64 << 10;
pub(super) const MAX_DEPTH: usize = 64;
const MAX_UTF8_BYTES_PER_UTF16_UNIT: usize = 3;
const PATH_SEPARATOR: char = '/';
pub(super) const MAX_PATH_BYTES: usize =
	MAX_DEPTH * (MAX_NAME_UNITS * MAX_UTF8_BYTES_PER_UTF16_UNIT + PATH_SEPARATOR.len_utf8());
pub(super) const MAX_CAPTURE_BYTES: usize = 64 << 20;
const PAGE_ENTRIES: NonZeroUsize = NonZeroUsize::MIN;

/// Failure to capture all supported metadata from a selected HFS+ volume.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CaptureError<E: Error + 'static> {
	#[error("HFS+ catalog could not be read")]
	Catalog(#[from] CatalogError<E>),
	#[error("HFS+ catalog was opened on another volume")]
	ForeignCatalog,
	#[error("HFS+ structure could not be validated")]
	Structure(#[from] CheckError<E>),
	#[error("HFS+ hard-link group could not be resolved")]
	HardLink(#[from] HardLinkError<E>),
	#[error("HFS+ stored fork could not be read")]
	Fork(#[from] OverflowError<E>),
	#[error("HFS+ attributes could not be read")]
	Attributes(#[from] AttributeReadError<E>),
	#[error("HFS+ rebuild needs a clean, unjournaled volume with supported header fields")]
	VolumeProfile,
	#[error("the HFS+ volume carries opaque boot or reserved sector data")]
	ReservedSectors,
	#[error("the HFS+ volume header carries Finder catalog references")]
	VolumeFinderReferences,
	#[error("the HFS+ source contains unsupported allocations or private catalog records")]
	StorageCoverage,
	#[error("the HFS+ overflow tree contains records that cannot be retained by a rebuild")]
	OverflowRecords,
	#[error("HFS+ entry {} carries unsupported catalog metadata", .0.0)]
	EntryMetadata(CatalogId),
	#[error("HFS+ entry {} cannot be admitted as an ordinary file, folder or symlink", .0.0)]
	EntryKind(CatalogId),
	#[error("HFS+ entry {} contains a Finder alias or layout requiring reference updates", .0.0)]
	FinderReferences(CatalogId),
	#[error("HFS+ entry {} contains a Finder alias or layout requiring reference updates", .id.0)]
	FinderLayout {
		id: CatalogId,
		#[source]
		error: StoreEditError,
	},
	#[error("HFS+ entry {} contains a Finder alias or layout exceeding the store byte budget", .0.0)]
	FinderStoreBudget(CatalogId),
	#[error("HFS+ catalog record {} has unsupported path syntax or spelling", .0.0)]
	CatalogName(CatalogId),
	#[error("an HFS+ name could not be normalized")]
	Normalize(#[from] NameError),
	#[error("HFS+ catalog path is unavailable in the edit manifest")]
	Path(#[from] VirtualPathError),
	#[error("HFS+ catalog contains duplicate edit path {0:?}")]
	DuplicatePath(VirtualPath),
	#[error("HFS+ attribute owner {} could not be assigned to a rebuilt record", .0.0)]
	AttributeOwner(CatalogId),
	#[error("HFS+ attribute for owner {} has an unsupported name or value", .owner.0)]
	AttributeValue {
		owner: CatalogId,
		#[source]
		error: VolumeError,
	},
	#[error("HFS+ attribute for owner {} has invalid UTF-16", .0.0)]
	AttributeName(CatalogId),
	#[error("HFS+ edit metadata exceeds the capture budget")]
	Budget,
	#[error("could not reserve HFS+ edit metadata")]
	Allocation(#[from] TryReserveError),
}

pub(super) fn capture<'volume, S: ReadAt>(
	volume: &'volume Volume<S>,
	catalog: &Catalog<'volume, S>,
) -> Result<Snapshot<'volume, S>, CaptureError<S::Error>>
where
	S::Error: Error + 'static,
{
	if !catalog.belongs_to(volume) {
		return Err(CaptureError::ForeignCatalog);
	}
	let header = volume.header();
	metadata::check_volume(header)?;
	if !volume
		.reserved_sectors_are_clear()
		.map_err(OverflowError::Read)?
	{
		return Err(CaptureError::ReservedSectors);
	}
	let limits = CheckLimits {
		max_catalog_entries: MAX_ENTRIES,
		..CheckLimits::default()
	};
	let report = volume.check_structure(limits)?;
	metadata::check_storage(&report)?;
	if !volume.overflow_tree_is_empty(limits.allocation.overflow)? {
		return Err(CaptureError::OverflowRecords);
	}
	let mut remaining = MAX_CAPTURE_BYTES;
	let mut attributes = AttributeInventory::read(volume, &mut remaining)?;
	let root = catalog.root()?;
	let name = stored_name(&root)?;
	let root_metadata = metadata::directory(&root)?;
	let root_attributes = attributes.take(&root.metadata)?;
	let mut capture = Capture {
		volume,
		catalog,
		limits,
		remaining,
		attributes,
		entries: BTreeMap::new(),
		linked: BTreeMap::new(),
		visible_dirs: 0,
		root_created: root_metadata.dates[0],
		finder_store_name: normalize(STORE_NAME)?,
	};
	capture.walk(root.metadata.id, "", 0)?;
	capture.check_inventory(&report)?;
	Ok(Snapshot {
		name,
		root: root_metadata,
		dates: header.dates,
		finder_info: header.finder_info,
		block_size: header.geometry.block_size(),
		entries: capture.entries,
		root_attributes,
	})
}

struct Capture<'catalog, 'volume, S> {
	volume: &'volume Volume<S>,
	catalog: &'catalog Catalog<'volume, S>,
	limits: CheckLimits,
	remaining: usize,
	attributes: AttributeInventory,
	entries: BTreeMap<VirtualPath, Node<'volume, S>>,
	linked: BTreeMap<CatalogId, SharedFile<'volume, S>>,
	visible_dirs: usize,
	root_created: u32,
	finder_store_name: NormalizedName,
}

struct SharedFile<'volume, S> {
	file: Rc<File<'volume, S>>,
	metadata: EntryMetadata,
	attributes: Attributes,
}

impl<'volume, S: ReadAt> Capture<'_, 'volume, S>
where
	S::Error: Error + 'static,
{
	fn walk(
		&mut self,
		id: CatalogId,
		parent: &str,
		depth: usize,
	) -> Result<(), CaptureError<S::Error>> {
		let mut directory = self.catalog.read_dir(id)?;
		loop {
			let page = directory.next_page(PAGE_ENTRIES)?;
			for entry in page.entries {
				if depth >= MAX_DEPTH || self.entries.len() >= MAX_ENTRIES {
					return Err(CaptureError::Budget);
				}
				let path = self.path(parent, &entry)?;
				let node = self.node(&entry)?;
				if matches!(node.kind, Kind::Directory) {
					self.visible_dirs += 1;
					self.walk(entry.metadata.id, path.as_str(), depth + 1)?;
				}
				match self.entries.entry(path) {
					MapEntry::Vacant(slot) => {
						slot.insert(node);
					}
					MapEntry::Occupied(slot) => {
						return Err(CaptureError::DuplicatePath(slot.key().clone()));
					}
				}
			}
			if page.complete {
				return Ok(());
			}
		}
	}

	fn path(&mut self, parent: &str, entry: &Entry) -> Result<VirtualPath, CaptureError<S::Error>> {
		let name = stored_name(entry)?;
		if name.contains(PATH_SEPARATOR) {
			return Err(CaptureError::CatalogName(entry.metadata.id));
		}
		let bytes = parent.len() + PATH_SEPARATOR.len_utf8() + name.len();
		if bytes > MAX_PATH_BYTES {
			return Err(CaptureError::Budget);
		}
		charge(
			&mut self.remaining,
			bytes + size_of::<Node<'volume, S>>() + size_of::<VirtualPath>(),
		)?;
		let mut path = String::new();
		path.try_reserve_exact(bytes)?;
		path.push_str(parent);
		path.push(PATH_SEPARATOR);
		path.push_str(&name);
		Ok(path.parse()?)
	}

	fn node(&mut self, entry: &Entry) -> Result<Node<'volume, S>, CaptureError<S::Error>> {
		let raw = &entry.metadata;
		let finder_store = self
			.catalog
			.collation()
			.compare(&entry.name, self.finder_store_name.as_units())
			.is_eq();
		let (metadata, kind) = match (entry.kind, raw.permissions.mode & BSD_TYPE_MASK) {
			(EntryKind::File { .. }, BSD_REGULAR_FILE) => {
				return self.regular_file(raw, finder_store);
			}
			_ if finder_store => return Err(CaptureError::FinderReferences(raw.id)),
			(EntryKind::Directory { .. }, BSD_DIRECTORY) => {
				(metadata::directory(entry)?, Kind::Directory)
			}
			(EntryKind::File { data, resource }, BSD_SYMLINK) => {
				let metadata = metadata::symlink(raw, &resource)?;
				let target = self.link(raw.id, &data)?;
				(metadata, Kind::Symlink(target))
			}
			_ => return Err(CaptureError::EntryKind(raw.id)),
		};
		Ok(Node {
			metadata,
			kind,
			attributes: self.attributes.take(raw)?.into(),
		})
	}

	fn link(&mut self, id: CatalogId, data: &Fork) -> Result<Vec<u8>, CaptureError<S::Error>> {
		let len = usize::try_from(data.logical_bytes)
			.ok()
			.filter(|len| (1..=MAX_SYMLINK_TARGET_BYTES).contains(len))
			.ok_or(CaptureError::EntryKind(id))?;
		charge(&mut self.remaining, len)?;
		let mut target = Vec::new();
		target.try_reserve_exact(len)?;
		target.resize(len, 0);
		let fork =
			self.volume
				.open_fork(id, ForkKind::Data, data, self.limits.allocation.overflow)?;
		fork.read_exact_at(ByteOffset(0), &mut target)
			.map_err(OverflowError::Read)?;
		if target.contains(&0) {
			return Err(CaptureError::EntryKind(id));
		}
		Ok(target)
	}

	fn regular_file(
		&mut self,
		raw: &Metadata,
		finder_store: bool,
	) -> Result<Node<'volume, S>, CaptureError<S::Error>> {
		let file = self
			.catalog
			.regular_file(raw.id, self.limits.hard_links)?
			.ok_or(CaptureError::EntryKind(raw.id))?;
		let link = file.hard_link();
		if finder_store && link.is_some() {
			return Err(CaptureError::FinderReferences(raw.id));
		}
		let metadata = metadata::regular_file(file.metadata(), link)?;
		let inode = if let Some(link) = link {
			metadata::check_link_stub(raw, link, self.root_created)?;
			Some(file.inode_id())
		} else {
			None
		};
		if let Some(shared) = inode.and_then(|id| self.linked.get(&id)) {
			if shared.metadata != metadata {
				return Err(CaptureError::EntryMetadata(file.inode_id()));
			}
			return Ok(Node {
				metadata,
				kind: Kind::File {
					file: Rc::clone(&shared.file),
					inode,
				},
				attributes: Rc::clone(&shared.attributes),
			});
		}
		charge(
			&mut self.remaining,
			size_of::<SharedFile<'volume, S>>() + size_of::<File<'volume, S>>(),
		)?;
		let attributes: Attributes = self.attributes.take(file.metadata())?.into();
		let overflow = self.limits.allocation.overflow;
		let data = file.open_stored_fork(ForkKind::Data, overflow)?;
		let resource = file.open_stored_fork(ForkKind::Resource, overflow)?;
		let layout = finder_store
			.then(|| finder::read_store(raw.id, &data, &mut self.remaining))
			.transpose()?;
		let file = Rc::new(File {
			data: Payload::image(data),
			resource: (!resource.is_empty()).then(|| Payload::image(resource)),
		});
		if let Some(id) = inode {
			self.linked.insert(
				id,
				SharedFile {
					file: Rc::clone(&file),
					metadata,
					attributes: Rc::clone(&attributes),
				},
			);
		}
		let kind = match layout {
			Some(layout) => Kind::FinderStore { file, layout },
			None => Kind::File { file, inode },
		};
		Ok(Node {
			metadata,
			kind,
			attributes,
		})
	}

	fn check_inventory(&self, report: &StructuralReport) -> Result<(), CaptureError<S::Error>> {
		self.attributes.check_consumed()?;
		let visible_files = self.entries.len() - self.visible_dirs;
		let private_folders = usize::from(!self.linked.is_empty());
		if self.linked.len() != report.hard_link_groups as usize
			|| visible_files + self.linked.len() != report.catalog_files as usize
			|| self.visible_dirs + private_folders != report.catalog_folders as usize
		{
			return Err(CaptureError::StorageCoverage);
		}
		Ok(())
	}
}

fn stored_name<E: Error + 'static>(entry: &Entry) -> Result<String, CaptureError<E>> {
	let normalized = normalize_utf16(&entry.name)?;
	if normalized.as_units() != entry.name {
		return Err(CaptureError::CatalogName(entry.metadata.id));
	}
	String::from_utf16(&entry.name).map_err(|_| CaptureError::CatalogName(entry.metadata.id))
}

fn charge<E: Error + 'static>(remaining: &mut usize, bytes: usize) -> Result<(), CaptureError<E>> {
	*remaining = remaining.checked_sub(bytes).ok_or(CaptureError::Budget)?;
	Ok(())
}
