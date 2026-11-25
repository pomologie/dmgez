use std::{
	cell::{BorrowMutError, RefCell},
	collections::TryReserveError,
	num::{NonZeroU16, NonZeroUsize},
};

use dmgez_vfs::{
	Disk, LogicalDisk, VolumeId, Window,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
		Metadata, NodeKind, RawName, VolumeBackend, VolumeView,
	},
};
use hfsplus_core::{ByteLen, ByteOffset, ReadAt};
use hfsplus_format::volume::CatalogId;
use hfsplus_reader::{
	Volume,
	catalog::{
		Catalog, CatalogError, CatalogLimits, Directory, Entry, EntryKind, HardLinkError,
		HardLinkLimits, RegularFile,
	},
	decmpfs::{LogicalDataError, LogicalDataLimits},
	overflow::{ForkKind as HfsFork, OverflowError, OverflowLimits},
};

const BSD_TYPE_MASK: u16 = 0o170000;
const BSD_REGULAR_FILE: u16 = 0o100000;
const BSD_SYMBOLIC_LINK: u16 = 0o120000;
const BSD_COMPRESSED: u8 = 1 << 5;
const LINK_FINDER_CODE: &[u8; 8] = b"slnkrhap";
const HFS_SYMLINK_LIMIT: NonZeroUsize = NonZeroUsize::new(1024).unwrap();

/// Independent limits for each engine operator and the materialized link payload.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
	pub catalog: CatalogLimits,
	pub hard_links: HardLinkLimits,
	pub stored_fork: OverflowLimits,
	pub logical_data: LogicalDataLimits,
	pub link_bytes: NonZeroUsize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			catalog: CatalogLimits::default(),
			hard_links: HardLinkLimits::default(),
			stored_fork: OverflowLimits::default(),
			logical_data: LogicalDataLimits::default(),
			link_bytes: HFS_SYMLINK_LIMIT,
		}
	}
}

/// Typed error from the selected extent or an HFS+ operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Selection(#[from] dmgez_vfs::Error),
	#[error(transparent)]
	Volume(#[from] hfsplus_reader::Error<SourceError>),
	#[error(transparent)]
	Catalog(#[from] CatalogError<SourceError>),
	#[error(transparent)]
	HardLink(#[from] HardLinkError<SourceError>),
	#[error(transparent)]
	StoredFork(#[from] OverflowError<SourceError>),
	#[error(transparent)]
	LogicalData(#[from] LogicalDataError<SourceError>),
	#[error(transparent)]
	Allocation(#[from] TryReserveError),
	#[error("unsupported HFS+ lookup encoding")]
	LookupEncoding,
	#[error("unsupported HFS+ fork selection")]
	ForkSelection,
	#[error("admitted regular file is missing")]
	MissingRegularFile,
	#[error("symbolic-link record does not match the supported HFS+ profile")]
	LinkProfile,
	#[error("symbolic-link payload exceeds the configured byte limit")]
	LinkLimit,
	#[error("symbolic-link payload is empty or contains NUL")]
	LinkPayload,
}

/// Bridge from the engine's shared positional source to a mutable VFS window.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
	#[error("filesystem read attempted to reenter its source")]
	Reentrant(#[from] BorrowMutError),
	#[error(transparent)]
	Window(#[from] dmgez_vfs::Error),
}

struct WindowSource<'disk, S> {
	window: RefCell<Window<'disk, S>>,
	length: ByteLen,
}

impl<S: LogicalDisk> ReadAt for WindowSource<'_, S> {
	type Error = SourceError;

	fn len(&self) -> ByteLen {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), SourceError> {
		Ok(self
			.window
			.try_borrow_mut()?
			.read_exact_at(offset.0, output)?)
	}
}

/// Backend admitted on a single selected partition or whole-disk volume.
pub struct HfsBackend<'volume, 'disk, S> {
	volume: &'volume Volume<WindowSource<'disk, S>>,
	catalog: Catalog<'volume, WindowSource<'disk, S>>,
	limits: Limits,
}

/// A bounded HFS+ directory cursor.
pub struct HfsDirectory<'catalog, 'volume, 'disk, S>(
	Directory<'catalog, 'volume, WindowSource<'disk, S>>,
);

/// An admitted read view whose source is borrowed from the selected disk.
pub type HfsView<'volume, 'disk, S> = VolumeView<HfsBackend<'volume, 'disk, S>>;

/// Opens the selected data extent and lends a view for one session.
///
/// # Errors
/// Reports selection and filesystem admission failures before invoking the callback.
pub fn with_view<S: LogicalDisk, T>(
	disk: &mut Disk<S>,
	selected: VolumeId,
	limits: Limits,
	read: impl FnOnce(&mut HfsView<'_, '_, S>) -> T,
) -> Result<T, Error> {
	let window = disk.data_window(selected)?;
	let source = WindowSource {
		length: ByteLen(window.len()),
		window: RefCell::new(window),
	};
	let volume = Volume::open(source)?;
	let catalog = volume.open_catalog(limits.catalog)?;
	let backend = HfsBackend {
		volume: &volume,
		catalog,
		limits,
	};
	Ok(read(&mut VolumeView::new(selected, backend)))
}

impl<'volume, 'disk, S: LogicalDisk> HfsBackend<'volume, 'disk, S> {
	fn regular_file(
		&self,
		node: CatalogId,
	) -> Result<RegularFile<'volume, WindowSource<'disk, S>>, Error> {
		self.catalog
			.regular_file(node, self.limits.hard_links)?
			.ok_or(Error::MissingRegularFile)
	}
}

impl<'volume, 'disk, S: LogicalDisk> VolumeBackend for HfsBackend<'volume, 'disk, S> {
	type Id = CatalogId;
	type Error = Error;
	type Directory<'view>
		= HfsDirectory<'view, 'volume, 'disk, S>
	where
		Self: 'view;

	fn root(&mut self) -> Result<CatalogId, Error> {
		Ok(self.catalog.root()?.metadata.id)
	}

	fn lookup(
		&mut self,
		parent: CatalogId,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<CatalogId>>, Error> {
		let child = match name {
			LookupName::Utf8(name) => self.catalog.lookup(parent, name),
			LookupName::StoredUtf16(name) => self.catalog.lookup_raw(parent, name),
			_ => return Err(Error::LookupEncoding),
		}?;
		Ok(child.map(visible_entry))
	}

	fn stat(&mut self, node: CatalogId) -> Result<Option<Metadata<u64>>, Error> {
		let Some(entry) = self.catalog.entry(node)? else {
			return Ok(None);
		};
		let kind = node_kind(&entry);
		let (permissions, hard_link) = if kind == NodeKind::RegularFile {
			let file = self.regular_file(node)?;
			let hard_link = file.hard_link().map(|link| HardLink {
				storage: u64::from(file.inode_id().0),
				count: link.count,
			});
			(file.metadata().permissions, hard_link)
		} else {
			(entry.metadata.permissions, None)
		};
		Ok(Some(Metadata {
			kind,
			mode: permissions.mode,
			uid: permissions.uid,
			gid: permissions.gid,
			hard_link,
		}))
	}

	fn read_dir(&mut self, directory: CatalogId) -> Result<Self::Directory<'_>, Error> {
		Ok(HfsDirectory(self.catalog.read_dir(directory)?))
	}

	fn with_fork<T>(
		&mut self,
		node: CatalogId,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = Error>) -> T,
	) -> Result<Option<T>, Error> {
		let file = self.regular_file(node)?;
		let result = match kind {
			ForkKind::Data => read(&mut ReadFork(
				file.open_logical_data(self.limits.logical_data)?,
			)),
			ForkKind::Resource => {
				let resource = file.open_stored_fork(HfsFork::Resource, self.limits.stored_fork)?;
				read(&mut ReadFork(resource))
			}
			_ => return Err(Error::ForkSelection),
		};
		Ok(Some(result))
	}

	fn read_link(&mut self, node: CatalogId) -> Result<Option<Vec<u8>>, Error> {
		let Some(entry) = self.catalog.entry(node)? else {
			return Ok(None);
		};
		let EntryKind::File { data, resource } = entry.kind else {
			return Err(Error::LinkProfile);
		};
		if node_kind(&entry) != NodeKind::Symlink
			|| !entry.metadata.finder_info.starts_with(LINK_FINDER_CODE)
			|| entry.metadata.permissions.owner_flags & BSD_COMPRESSED != 0
			|| resource.logical_bytes != 0
			|| resource.total_blocks != 0
		{
			return Err(Error::LinkProfile);
		}
		let size = usize::try_from(data.logical_bytes).map_err(|_| Error::LinkLimit)?;
		if size > self.limits.link_bytes.get().min(HFS_SYMLINK_LIMIT.get()) {
			return Err(Error::LinkLimit);
		}
		let fork = self
			.volume
			.open_fork(node, HfsFork::Data, &data, self.limits.stored_fork)?;
		let mut bytes = Vec::new();
		bytes.try_reserve_exact(size)?;
		bytes.resize(size, 0);
		fork.read_exact_at(ByteOffset(0), &mut bytes)?;
		if bytes.is_empty() || bytes.contains(&0) {
			return Err(Error::LinkPayload);
		}
		Ok(Some(bytes))
	}
}

impl<S: LogicalDisk> DirectoryReader for HfsDirectory<'_, '_, '_, S> {
	type Id = CatalogId;
	type Error = Error;

	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<CatalogId>, Error> {
		let page = self.0.next_page(limit.into())?;
		Ok(DirectoryPage {
			entries: page.entries.into_iter().map(visible_entry).collect(),
			complete: page.complete,
		})
	}
}

struct ReadFork<F>(F);

impl<F: ReadAt> ForkReader for ReadFork<F>
where
	Error: From<F::Error>,
{
	type Error = Error;

	fn len(&self) -> u64 {
		self.0.len().0
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		Ok(self.0.read_exact_at(ByteOffset(offset), output)?)
	}
}

fn visible_entry(entry: Entry) -> DirectoryEntry<CatalogId> {
	DirectoryEntry {
		kind: node_kind(&entry),
		name: RawName::Utf16(entry.name),
		node: entry.metadata.id,
	}
}

fn node_kind(entry: &Entry) -> NodeKind {
	if matches!(entry.kind, EntryKind::Directory { .. }) {
		return NodeKind::Directory;
	}
	match entry.metadata.permissions.mode & BSD_TYPE_MASK {
		BSD_REGULAR_FILE => NodeKind::RegularFile,
		BSD_SYMBOLIC_LINK => NodeKind::Symlink,
		_ => NodeKind::Other,
	}
}
