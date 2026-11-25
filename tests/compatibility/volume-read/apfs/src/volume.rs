use std::{
	cell::RefCell,
	error::Error as StdError,
	num::{NonZeroU16, NonZeroU32},
};

use apfs_core::{ByteOffset, ReadAt};
use apfs_format::object::ObjectId;
use apfs_reader::{
	DirectoryCursor, DirectoryEntry as NativeEntry, Error as NativeError, FileData, FileKind,
	InodeCount, InodeId, VolumeView as NativeView, XattrData,
};
use dmgez_vfs::{
	Error as DiskError, LogicalDisk, Window,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
		Metadata, NodeKind, RawName, VolumeBackend,
	},
};
use thiserror::Error;

/// Mutable logical read cache behind APFS's immutable-source interface.
pub struct WindowSource<'disk, S> {
	length: u64,
	window: RefCell<Window<'disk, S>>,
}

impl<'disk, S: LogicalDisk> WindowSource<'disk, S> {
	pub fn new(window: Window<'disk, S>) -> Self {
		Self {
			length: window.len(),
			window: RefCell::new(window),
		}
	}
}

#[derive(Debug, Error)]
pub enum SourceError {
	#[error("APFS window already has an active positional read")]
	ActiveRead,
	#[error("APFS window read failed")]
	Disk(#[from] DiskError),
}

impl<S: LogicalDisk> ReadAt for WindowSource<'_, S> {
	type Error = SourceError;

	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		self.window
			.try_borrow_mut()
			.map_err(|_| SourceError::ActiveRead)?
			.read_exact_at(offset.0, output)?;
		Ok(())
	}
}

/// Visible identity retains the APFS sibling ID while forks use the inode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Node {
	inode: InodeId,
	sibling: Option<ObjectId>,
}

#[derive(Debug, Error)]
pub enum AdapterError<E: StdError + 'static> {
	#[error("APFS reader rejected the operation")]
	Reader(#[from] NativeError<E>),
	#[error("APFS stored names require UTF-8")]
	NameEncoding,
	#[error("APFS directory entry changed or could not be revalidated")]
	DirectoryEntry,
	#[error("APFS sibling is outside the checked hard-link group")]
	LinkIdentity,
	#[error("APFS fork selection is unsupported")]
	ForkKind,
	#[error("could not allocate the checked directory page")]
	Allocation,
}

pub struct ApfsBackend<'source, S> {
	view: NativeView<'source, S>,
}

impl<'source, S> ApfsBackend<'source, S> {
	pub fn new(view: NativeView<'source, S>) -> Self {
		Self { view }
	}
}

impl<'source, S> VolumeBackend for ApfsBackend<'source, S>
where
	S: ReadAt,
	S::Error: StdError + 'static,
{
	type Id = Node;
	type Error = AdapterError<S::Error>;
	type Directory<'view>
		= ApfsDirectory<'view, 'source, S>
	where
		Self: 'view;

	fn root(&mut self) -> Result<Node, Self::Error> {
		Ok(Node {
			inode: self.view.root().id,
			sibling: None,
		})
	}

	fn lookup(
		&mut self,
		parent: Node,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<Node>>, Self::Error> {
		let LookupName::Utf8(name) = name else {
			return Err(AdapterError::NameEncoding);
		};
		Ok(self
			.view
			.lookup_stored_name(parent.inode, name)?
			.map(directory_entry))
	}

	fn stat(&mut self, node: Node) -> Result<Option<Metadata<u64>>, Self::Error> {
		let Some(inode) = self.view.stat(node.inode)? else {
			return Ok(None);
		};
		let kind = node_kind(inode.kind);
		let needs_link = node.sibling.is_some()
			|| (kind == NodeKind::RegularFile
				&& matches!(inode.count, InodeCount::Links(n) if n > 1));
		let hard_link = if needs_link {
			let InodeCount::Links(count) = inode.count else {
				return Err(AdapterError::LinkIdentity);
			};
			let count = NonZeroU32::new(count).ok_or(AdapterError::LinkIdentity)?;
			let sibling = node.sibling.ok_or(AdapterError::LinkIdentity)?;
			let group = self
				.view
				.hard_links(node.inode)?
				.ok_or(AdapterError::LinkIdentity)?;
			if !group.iter().any(|link| link.sibling.as_raw() == sibling.0) {
				return Err(AdapterError::LinkIdentity);
			}
			Some(HardLink {
				storage: inode.id.as_raw(),
				count,
			})
		} else {
			None
		};
		Ok(Some(Metadata {
			kind,
			mode: inode.mode,
			uid: inode.owner,
			gid: inode.group,
			hard_link,
		}))
	}

	fn read_dir(&mut self, directory: Node) -> Result<Self::Directory<'_>, Self::Error> {
		Ok(ApfsDirectory {
			view: &mut self.view,
			directory: directory.inode,
			cursor: None,
		})
	}

	fn with_fork<T>(
		&mut self,
		node: Node,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> Result<Option<T>, Self::Error> {
		let fork = match kind {
			ForkKind::Data => self.view.open_file(node.inode)?.map(ApfsFork::Data),
			ForkKind::Resource => self
				.view
				.open_resource_fork(node.inode)?
				.map(ApfsFork::Resource),
			_ => return Err(AdapterError::ForkKind),
		};
		Ok(fork.map(|mut stream| read(&mut stream)))
	}

	fn read_link(&mut self, node: Node) -> Result<Option<Vec<u8>>, Self::Error> {
		Ok(self.view.read_link(node.inode)?)
	}
}

pub struct ApfsDirectory<'view, 'source, S> {
	view: &'view mut NativeView<'source, S>,
	directory: InodeId,
	cursor: Option<DirectoryCursor>,
}

impl<S> DirectoryReader for ApfsDirectory<'_, '_, S>
where
	S: ReadAt,
	S::Error: StdError + 'static,
{
	type Id = Node;
	type Error = AdapterError<S::Error>;

	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<Node>, Self::Error> {
		let page = self.view.read_dir(self.directory, self.cursor, limit)?;
		let mut entries = Vec::new();
		entries
			.try_reserve_exact(page.entries.len())
			.map_err(|_| AdapterError::Allocation)?;
		for entry in page.entries {
			// Native paging supplies stored directory records. Check their targets
			// before binding opaque handles, just as in the one-name lookup path.
			if self
				.view
				.lookup_stored_name(self.directory, &entry.name)?
				.as_ref() != Some(&entry)
			{
				return Err(AdapterError::DirectoryEntry);
			}
			entries.push(directory_entry(entry));
		}
		self.cursor = page.next;
		Ok(DirectoryPage {
			entries,
			complete: page.next.is_none(),
		})
	}
}

enum ApfsFork<'view, 'source, S> {
	Data(FileData<'view, 'source, S>),
	Resource(XattrData<'view, 'source, S>),
}

impl<S> ForkReader for ApfsFork<'_, '_, S>
where
	S: ReadAt,
	S::Error: StdError + 'static,
{
	type Error = AdapterError<S::Error>;

	fn len(&self) -> u64 {
		match self {
			Self::Data(data) => data.len(),
			Self::Resource(data) => data.len(),
		}
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Self::Error> {
		match self {
			Self::Data(data) => data.read_exact_at(ByteOffset(offset), output)?,
			Self::Resource(data) => data.read_exact_at(ByteOffset(offset), output)?,
		}
		Ok(())
	}
}

fn directory_entry(entry: NativeEntry) -> DirectoryEntry<Node> {
	DirectoryEntry {
		name: RawName::Utf8(entry.name),
		node: Node {
			inode: entry.inode,
			sibling: entry.sibling,
		},
		kind: node_kind(entry.kind),
	}
}

fn node_kind(kind: FileKind) -> NodeKind {
	match kind {
		FileKind::Directory => NodeKind::Directory,
		FileKind::Regular => NodeKind::RegularFile,
		FileKind::Symlink => NodeKind::Symlink,
		_ => NodeKind::Other,
	}
}
