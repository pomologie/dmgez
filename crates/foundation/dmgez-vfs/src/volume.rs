//! Read-only filesystem views over one admitted source and selected volume (C5).

use std::{
	error::Error as StdError,
	fmt,
	hash::{Hash, Hasher},
	num::{NonZeroU16, NonZeroU32},
	ptr,
	sync::Arc,
};

use thiserror::Error;

use crate::VolumeId;

mod fork;

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

/// Engine operator that advances one bounded directory scan.
pub trait DirectoryReader {
	type Id: Copy;
	type Error: StdError + 'static;

	/// Returns at most `limit` entries. Engine scan budgets also apply.
	///
	/// # Errors
	/// Returns the cause of a failed page and retains the prior cursor position.
	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<Self::Id>, Self::Error>;
}

/// Filesystem engine adapter bound to one immutable volume and source.
/// Operations must preserve the source identity and enforce engine I/O budgets.
/// The engine interprets native names, on-disk object kinds, and fork formats.
pub trait VolumeBackend {
	type Id: Copy;
	type Error: StdError + 'static;
	type Directory<'view>: DirectoryReader<Id = Self::Id, Error = Self::Error>
	where
		Self: 'view;

	/// Resolves the visible root directory, validating it as necessary.
	fn root(&mut self) -> Result<Self::Id, Self::Error>;

	/// A numeric engine-native identity, when its identifier has such a representation.
	fn numeric_id(&self, _node: Self::Id) -> Option<u64> {
		None
	}

	/// Finds one child in a directory without resolving a symbolic link.
	fn lookup(
		&mut self,
		parent: Self::Id,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<Self::Id>>, Self::Error>;

	/// Gets effective metadata. Hard-link storage keys are local to this view.
	fn stat(&mut self, node: Self::Id) -> Result<Option<Metadata<u64>>, Self::Error>;

	/// Opens a scan over the selected directory's visible children.
	fn read_dir(&mut self, directory: Self::Id) -> Result<Self::Directory<'_>, Self::Error>;

	/// Lends exactly the selected fork to the callback for this operation.
	/// A `None` result means the fork is absent. A present empty fork lends a
	/// zero-length reader. This operation never follows symbolic links.
	fn with_fork<T>(
		&mut self,
		node: Self::Id,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> Result<Option<T>, Self::Error>;

	/// Returns the exact symbolic link payload without opening its target.
	fn read_link(&mut self, node: Self::Id) -> Result<Option<Vec<u8>>, Self::Error>;
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
