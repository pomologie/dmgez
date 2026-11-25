use std::{
	collections::TryReserveError,
	num::{NonZeroU16, NonZeroU32, NonZeroUsize},
};

use dmgez_vfs::{
	Disk, LogicalDisk, VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, FinderInfo, ForkKind, ForkReader, HardLink,
		LookupName, Metadata, NativeNameKey, NativeNameProfile, NodeKind, Quarantine,
		QuarantineScope, RawName, VolumeBackend, VolumeView,
	},
};
use hfsplus_core::{ByteOffset, ReadAt, names::NameError};
pub use hfsplus_format::volume::Signature as VolumeSignature;
use hfsplus_format::{bsd, symlink::MAX_SYMLINK_TARGET_BYTES, volume::CatalogId};
use hfsplus_reader::{
	Volume,
	attributes::AttributeReadError,
	catalog::{
		Catalog, CatalogError, CatalogLimits, Directory, Entry, EntryKind, HardLinkError,
		HardLinkLimits, RegularFile, SymlinkError,
	},
	decmpfs::{LogicalDataError, LogicalDataLimits},
	overflow::{ForkKind as HfsFork, OverflowError, OverflowLimits},
};

mod finder_info;
mod native_names;
mod quarantine;
mod session;
mod xattrs;

pub use session::{SourceError, WindowSource, with_catalog};

const HFS_SYMLINK_LIMIT: NonZeroUsize = NonZeroUsize::new(MAX_SYMLINK_TARGET_BYTES).unwrap();
const DEFAULT_READ_REQUEST_LIMIT: u64 = 1_000_000;
const DEFAULT_READ_BYTE_LIMIT: u64 = 1 << 32;

/// Per-operator engine limits and aggregate limits for one admitted volume session.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
	pub catalog: CatalogLimits,
	pub hard_links: HardLinkLimits,
	pub stored_fork: OverflowLimits,
	pub logical_data: LogicalDataLimits,
	pub link_bytes: NonZeroUsize,
	/// Number of requested positional reads, including failed and empty reads.
	pub max_read_requests: u64,
	/// Total decoded bytes requested from the bounded volume window.
	pub max_read_bytes: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			catalog: CatalogLimits::default(),
			hard_links: HardLinkLimits::default(),
			stored_fork: OverflowLimits::default(),
			logical_data: LogicalDataLimits::default(),
			link_bytes: HFS_SYMLINK_LIMIT,
			max_read_requests: DEFAULT_READ_REQUEST_LIMIT,
			max_read_bytes: DEFAULT_READ_BYTE_LIMIT,
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
	Attributes(#[from] AttributeReadError<SourceError>),
	#[error(transparent)]
	CatalogAttributes(hfsplus_reader::catalog::CatalogAttributeError<SourceError>),
	#[error(transparent)]
	Allocation(#[from] TryReserveError),
	#[error("unsupported HFS+ lookup encoding")]
	LookupEncoding,
	#[error("could not key a stored HFS+ name")]
	NativeName(#[from] NameError),
	#[error("unsupported HFS+ fork selection")]
	ForkSelection,
	#[error("admitted HFS+ attribute owner is missing")]
	MissingAttributeOwner,
	#[error("admitted HFS+ FinderInfo owner is missing")]
	MissingFinderInfoOwner,
	#[error(transparent)]
	FinderInfo(hfsplus_reader::catalog::FinderInfoError<SourceError>),
	#[error("the visible HFS+ hard link and its inode both own the same attribute name")]
	AmbiguousAttributeOwner,
	#[error("admitted regular file is missing")]
	MissingRegularFile,
	#[error("symbolic-link record does not match the supported HFS+ profile")]
	LinkProfile,
	#[error("symbolic-link payload exceeds the configured byte limit")]
	LinkLimit,
	#[error("symbolic-link payload is empty or contains NUL")]
	LinkPayload,
	#[error(transparent)]
	Symlink(SymlinkError<SourceError>),
}

impl From<SymlinkError<SourceError>> for Error {
	fn from(error: SymlinkError<SourceError>) -> Self {
		match error {
			SymlinkError::Catalog(error) => Self::Catalog(error),
			SymlinkError::UnsupportedProfile => Self::LinkProfile,
			SymlinkError::TargetLimit { .. } => Self::LinkLimit,
			SymlinkError::InvalidTarget(_) => Self::LinkPayload,
			SymlinkError::Fork(error) => Self::StoredFork(error),
			SymlinkError::Allocation(error) => Self::Allocation(error),
			other => Self::Symlink(other),
		}
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
	with_view_signature(disk, selected, limits, |view, _| read(view))
}

/// Opens the selected data extent and reports its admitted HFS+ or HFSX signature.
///
/// # Errors
/// Reports selection and filesystem admission failures before invoking the callback.
pub fn with_view_signature<S: LogicalDisk, T>(
	disk: &mut Disk<S>,
	selected: VolumeId,
	limits: Limits,
	read: impl FnOnce(&mut HfsView<'_, '_, S>, VolumeSignature) -> T,
) -> Result<T, Error> {
	with_catalog(disk, selected, limits, |volume, catalog| {
		read(
			&mut view_for_catalog(selected, volume, catalog, limits),
			volume.header().signature,
		)
	})
}

pub(crate) fn view_for_catalog<'volume, 'disk, S: LogicalDisk>(
	selected: VolumeId,
	volume: &'volume Volume<WindowSource<'disk, S>>,
	catalog: Catalog<'volume, WindowSource<'disk, S>>,
	limits: Limits,
) -> HfsView<'volume, 'disk, S> {
	VolumeView::new(
		selected,
		HfsBackend {
			volume,
			catalog,
			limits,
		},
	)
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

	fn numeric_id(&self, id: CatalogId) -> Option<u64> {
		Some(u64::from(id.0))
	}

	fn native_name_profile(&self) -> Option<NativeNameProfile> {
		native_names::profile(self.catalog.collation())
	}

	fn native_name_key(&self, name: LookupName<'_>) -> Result<Option<NativeNameKey>, Error> {
		native_names::key(self.catalog.collation(), name)
	}

	fn quarantine(&mut self, node: CatalogId, scope: QuarantineScope) -> Result<Quarantine, Error> {
		self.check_quarantine(node, scope)
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

	fn finder_info(&mut self, node: CatalogId) -> Result<FinderInfo, Error> {
		self.effective_finder_info(node)
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

	fn list_xattrs(
		&mut self,
		node: CatalogId,
		limit: NonZeroU32,
	) -> Result<Option<Vec<RawName>>, Error> {
		self.extended_attribute_names(node, limit).map(Some)
	}

	fn with_xattr<T>(
		&mut self,
		node: CatalogId,
		name: LookupName<'_>,
		read: impl FnOnce(&mut dyn ForkReader<Error = Error>) -> T,
	) -> Result<Option<T>, Error> {
		self.with_extended_attribute(node, name, read)
	}

	fn read_link(&mut self, node: CatalogId) -> Result<Option<Vec<u8>>, Error> {
		Ok(self
			.catalog
			.symlink(node, self.limits.link_bytes)?
			.map(|symlink| symlink.read(self.limits.stored_fork))
			.transpose()?)
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

pub(crate) fn node_kind(entry: &Entry) -> NodeKind {
	if matches!(entry.kind, EntryKind::Directory { .. }) {
		return NodeKind::Directory;
	}
	match entry.metadata.permissions.mode & bsd::TYPE_MASK {
		bsd::REGULAR_FILE => NodeKind::RegularFile,
		bsd::SYMLINK => NodeKind::Symlink,
		_ => NodeKind::Other,
	}
}
