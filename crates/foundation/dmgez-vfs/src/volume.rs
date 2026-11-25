//! Read-only filesystem views over one admitted source and selected volume (C5).

use std::{
	fmt,
	hash::{Hash, Hasher},
	num::{NonZeroU16, NonZeroU32},
	ptr,
	sync::Arc,
};

use thiserror::Error;

use crate::VolumeId;

mod backend;
mod fork;

pub use backend::{DirectoryReader, VolumeBackend};
pub use fork::{Fork, ForkReader};

/// A native name retained without Unicode replacement or path interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RawName {
	Utf8(String),
	Utf16(Vec<u16>),
}

/// One component matched using the selected engine's admitted name policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LookupName<'name> {
	Utf8(&'name str),
	StoredUtf16(&'name [u16]),
}

impl RawName {
	/// Reuses a stored name as a borrowed lookup component.
	#[must_use]
	pub fn as_lookup(&self) -> LookupName<'_> {
		match self {
			Self::Utf8(name) => LookupName::Utf8(name),
			Self::Utf16(name) => LookupName::StoredUtf16(name),
		}
	}
}

/// Object kind in the visible filesystem namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NodeKind {
	Directory,
	RegularFile,
	Symlink,
	Other,
}

/// Direct child entry. Its name is separate from its object identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry<Id> {
	pub name: RawName,
	pub node: Id,
	pub kind: NodeKind,
}

/// A page in the engine's native directory order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage<Id> {
	pub entries: Vec<DirectoryEntry<Id>>,
	pub complete: bool,
}

/// Effective hard-link identity and the count admitted by the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardLink<Storage> {
	pub storage: Storage,
	pub count: NonZeroU32,
}

/// Shared POSIX metadata for one visible object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata<Storage> {
	pub kind: NodeKind,
	/// Stored POSIX mode, including object type bits when present.
	pub mode: u16,
	pub uid: u32,
	pub gid: u32,
	/// A storage identity is returned only when link resolution admits it.
	pub hard_link: Option<HardLink<Storage>>,
}

/// A separately selected byte stream of a regular file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkKind {
	/// Logical file content, including any codec that the engine supports.
	Data,
	/// Exact stored resource fork bytes.
	Resource,
}

/// Whether the checked source objects carry a recorded quarantine marker.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Quarantine {
	Clear,
	Marked,
	Unavailable,
}

/// Scope for a quarantine marker check.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum QuarantineScope {
	Object,
	/// Check the object and every containing directory through the volume root.
	Ancestors,
}

#[derive(Clone)]
struct Session(Arc<()>);

impl fmt::Debug for Session {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("Session")
	}
}

impl PartialEq for Session {
	fn eq(&self, other: &Self) -> bool {
		Arc::ptr_eq(&self.0, &other.0)
	}
}

impl Eq for Session {}

impl Hash for Session {
	fn hash<H: Hasher>(&self, state: &mut H) {
		ptr::hash(Arc::as_ptr(&self.0), state);
	}
}

/// Opaque visible object handle, valid only in the view that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeHandle<Id> {
	local: Id,
	kind: NodeKind,
	session: Session,
}

impl<Id: Copy> NodeHandle<Id> {
	/// The stored engine identifier reported by this handle's view.
	#[must_use]
	pub const fn native_id(&self) -> Id {
		self.local
	}
}

/// A visible directory entry whose handle belongs to the opened view.
pub type ViewEntry<Id> = DirectoryEntry<NodeHandle<Id>>;

/// Validated storage-group identity. It cannot open a private inode or fork.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageId {
	local: u64,
	session: Session,
}

impl StorageId {
	/// The stored engine identifier of this hard-link group within its view.
	#[must_use]
	pub const fn native_id(&self) -> u64 {
		self.local
	}
}

/// Failure accessing a selected filesystem view.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ViewError<E> {
	#[error("node belongs to a different volume view")]
	ForeignHandle,
	#[error("expected {expected:?}, found {actual:?}")]
	WrongKind {
		expected: NodeKind,
		actual: NodeKind,
	},
	#[error("admitted node is unavailable in its filesystem view")]
	MissingNode,
	#[error("admitted symbolic link has no target payload")]
	MissingLink,
	#[error("filesystem metadata disagrees with the admitted node kind")]
	InconsistentKind,
	#[error("filesystem returned {entries} directory entries for a limit of {limit}")]
	PageLimit { limit: u16, entries: usize },
	#[error("filesystem returned {entries} extended attributes for a limit of {limit}")]
	AttributeLimit { limit: u32, entries: usize },
	#[error("fork range {offset} + {len} exceeds {available} bytes")]
	OutOfRange {
		offset: u64,
		len: u64,
		available: u64,
	},
	#[error("filesystem backend failed")]
	Backend(#[source] E),
}

/// An opened read-only volume with a fresh handle namespace.
pub struct VolumeView<B> {
	partition: VolumeId,
	session: Session,
	backend: B,
}

impl<B: VolumeBackend> VolumeView<B> {
	/// Wraps an engine already opened on this selected partition or whole-disk
	/// extent. Multiple views on a partition have separate handle namespaces.
	#[must_use]
	pub fn new(partition: VolumeId, backend: B) -> Self {
		Self {
			partition,
			session: Session(Arc::new(())),
			backend,
		}
	}

	/// Partition selector already used to admit this view's source.
	#[must_use]
	pub const fn partition(&self) -> VolumeId {
		self.partition
	}

	/// Returns an opaque handle for the checked root directory.
	///
	/// # Errors
	/// Returns an error if the backend cannot resolve its root directory.
	pub fn root(&mut self) -> Result<NodeHandle<B::Id>, ViewError<B::Error>> {
		let local = self.backend.root().map_err(ViewError::Backend)?;
		Ok(NodeHandle {
			local,
			kind: NodeKind::Directory,
			session: self.session.clone(),
		})
	}

	/// Finds a child and returns its native name and an opaque handle.
	///
	/// # Errors
	/// Rejects foreign or non-directory parents and propagates backend errors.
	pub fn lookup(
		&mut self,
		parent: &NodeHandle<B::Id>,
		name: LookupName<'_>,
	) -> Result<Option<ViewEntry<B::Id>>, ViewError<B::Error>> {
		let parent = self.require_kind(parent, NodeKind::Directory)?;
		let entry = self
			.backend
			.lookup(parent, name)
			.map_err(ViewError::Backend)?;
		Ok(entry.map(|entry| bind_entry(entry, &self.session)))
	}

	/// Reports the admitted object identifier in numeric form when supported.
	/// # Errors
	/// Rejects an object handle from another view.
	pub fn numeric_id(&self, node: &NodeHandle<B::Id>) -> Result<Option<u64>, ViewError<B::Error>> {
		Ok(self.backend.numeric_id(self.require_session(node)?))
	}

	/// Checks the recorded quarantine state of an admitted object.
	///
	/// # Errors
	/// Rejects foreign handles and propagates a backend attribute or lookup failure.
	pub fn quarantine(
		&mut self,
		node: &NodeHandle<B::Id>,
		scope: QuarantineScope,
	) -> Result<Quarantine, ViewError<B::Error>> {
		let local = self.require_session(node)?;
		self.backend
			.quarantine(local, scope)
			.map_err(ViewError::Backend)
	}

	/// Gets effective metadata with any storage group scoped to this view.
	///
	/// # Errors
	/// Rejects foreign, missing, or inconsistent objects and propagates backend errors.
	pub fn stat(
		&mut self,
		node: &NodeHandle<B::Id>,
	) -> Result<Metadata<StorageId>, ViewError<B::Error>> {
		let local = self.require_session(node)?;
		let metadata = self
			.backend
			.stat(local)
			.map_err(ViewError::Backend)?
			.ok_or(ViewError::MissingNode)?;
		if metadata.kind != node.kind {
			return Err(ViewError::InconsistentKind);
		}
		Ok(Metadata {
			kind: metadata.kind,
			mode: metadata.mode,
			uid: metadata.uid,
			gid: metadata.gid,
			hard_link: metadata.hard_link.map(|link| HardLink {
				storage: StorageId {
					local: link.storage,
					session: self.session.clone(),
				},
				count: link.count,
			}),
		})
	}

	/// Opens a borrowed, bounded directory scan in this view.
	///
	/// # Errors
	/// Rejects foreign or non-directory handles and propagates backend errors.
	pub fn read_dir(
		&mut self,
		directory: &NodeHandle<B::Id>,
	) -> Result<DirectoryView<B::Directory<'_>>, ViewError<B::Error>> {
		let local = self.require_kind(directory, NodeKind::Directory)?;
		let reader = self.backend.read_dir(local).map_err(ViewError::Backend)?;
		Ok(DirectoryView {
			reader,
			session: self.session.clone(),
			complete: false,
		})
	}

	/// Opens a regular file's chosen fork for the duration of the callback.
	/// Returns `None` only when the engine reports the requested fork absent.
	/// Callback results are returned inside the result of opening the fork.
	///
	/// # Errors
	/// Rejects foreign or non-regular objects and propagates backend open errors.
	pub fn with_fork<T>(
		&mut self,
		node: &NodeHandle<B::Id>,
		kind: ForkKind,
		read: impl FnOnce(&mut Fork<'_, B::Error>) -> T,
	) -> Result<Option<T>, ViewError<B::Error>> {
		let local = self.require_kind(node, NodeKind::RegularFile)?;
		self.backend
			.with_fork(local, kind, |reader| read(&mut Fork::new(reader)))
			.map_err(ViewError::Backend)
	}

	/// Lists the exact stored extended-attribute names for one admitted object.
	/// `None` means enumeration is unavailable. An empty list means there are no attributes.
	///
	/// # Errors
	/// Rejects foreign handles, oversized listings, and backend metadata failures.
	pub fn list_xattrs(
		&mut self,
		node: &NodeHandle<B::Id>,
		limit: NonZeroU32,
	) -> Result<Option<Vec<RawName>>, ViewError<B::Error>> {
		let local = self.require_session(node)?;
		let names = self
			.backend
			.list_xattrs(local, limit)
			.map_err(ViewError::Backend)?;
		if let Some(entries) = names.as_ref().map(Vec::len)
			&& entries > limit.get() as usize
		{
			return Err(ViewError::AttributeLimit {
				limit: limit.get(),
				entries,
			});
		}
		Ok(names)
	}

	/// Opens an exact stored attribute value without following symbolic links.
	/// The metadata and bytes of a present empty attribute remain accessible.
	///
	/// # Errors
	/// Rejects foreign handles and propagates backend read or admission failures.
	pub fn with_xattr<T>(
		&mut self,
		node: &NodeHandle<B::Id>,
		name: LookupName<'_>,
		read: impl FnOnce(&mut Fork<'_, B::Error>) -> T,
	) -> Result<Option<T>, ViewError<B::Error>> {
		let local = self.require_session(node)?;
		self.backend
			.with_xattr(local, name, |reader| read(&mut Fork::new(reader)))
			.map_err(ViewError::Backend)
	}

	/// Gets the bytes of a symbolic link payload.
	///
	/// # Errors
	/// Rejects foreign or non-symlink handles, missing targets and backend errors.
	pub fn read_link(&mut self, node: &NodeHandle<B::Id>) -> Result<Vec<u8>, ViewError<B::Error>> {
		let local = self.require_kind(node, NodeKind::Symlink)?;
		self.backend
			.read_link(local)
			.map_err(ViewError::Backend)?
			.ok_or(ViewError::MissingLink)
	}

	fn require_session(&self, node: &NodeHandle<B::Id>) -> Result<B::Id, ViewError<B::Error>> {
		if self.session != node.session {
			return Err(ViewError::ForeignHandle);
		}
		Ok(node.native_id())
	}

	fn require_kind(
		&self,
		node: &NodeHandle<B::Id>,
		expected: NodeKind,
	) -> Result<B::Id, ViewError<B::Error>> {
		let local = self.require_session(node)?;
		if node.kind != expected {
			return Err(ViewError::WrongKind {
				expected,
				actual: node.kind,
			});
		}
		Ok(local)
	}
}

/// One session-bound directory scan, with separately bounded pages.
pub struct DirectoryView<D> {
	reader: D,
	session: Session,
	complete: bool,
}

impl<D: DirectoryReader> DirectoryView<D> {
	/// Returns a page and marks the scan complete after the engine reaches its end.
	///
	/// # Errors
	/// Rejects a page exceeding the requested size and propagates engine failures.
	pub fn next_page(
		&mut self,
		limit: NonZeroU16,
	) -> Result<DirectoryPage<NodeHandle<D::Id>>, ViewError<D::Error>> {
		if self.complete {
			return Ok(DirectoryPage {
				entries: Vec::new(),
				complete: true,
			});
		}
		let page = self.reader.next_page(limit).map_err(ViewError::Backend)?;
		if page.entries.len() > usize::from(limit.get()) {
			return Err(ViewError::PageLimit {
				limit: limit.get(),
				entries: page.entries.len(),
			});
		}
		self.complete = page.complete;
		let entries = page
			.entries
			.into_iter()
			.map(|entry| bind_entry(entry, &self.session))
			.collect();
		Ok(DirectoryPage {
			entries,
			complete: self.complete,
		})
	}
}

fn bind_entry<Id>(entry: DirectoryEntry<Id>, session: &Session) -> DirectoryEntry<NodeHandle<Id>> {
	DirectoryEntry {
		name: entry.name,
		node: NodeHandle {
			local: entry.node,
			kind: entry.kind,
			session: session.clone(),
		},
		kind: entry.kind,
	}
}
