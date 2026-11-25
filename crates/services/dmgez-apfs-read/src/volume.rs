use std::{
	error::Error as StdError,
	num::{NonZeroU16, NonZeroU32},
};

use apfs_core::{ByteOffset, ReadAt};
use apfs_format::object::ObjectId;
use apfs_reader::{
	DirectoryCursor, DirectoryEntry as NativeEntry, Error as NativeError, FileData, FileKind,
	InodeCount, InodeId, VolumeView as NativeView, XattrData,
};
use dmgez_vfs::volume::{
	DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
	Metadata, NodeKind, Quarantine, QuarantineScope, RawName, VolumeBackend,
};
use thiserror::Error;

use crate::VolumeInfo;

mod quarantine;
mod xattrs;

/// Visible identity retains the APFS sibling ID and its admitted parent.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Node {
	inode: InodeId,
	sibling: Option<ObjectId>,
	parent: Option<InodeId>,
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
	#[error("the admitted APFS attribute owner is missing")]
	MissingAttributeOwner,
	#[error("APFS attribute count exceeds the requested limit of {limit}")]
	AttributeLimit { limit: NonZeroU32 },
	#[error("could not allocate the checked APFS attribute list")]
	AttributeAllocation,
}

/// VFS adapter for a checked APFS volume view.
pub struct ApfsBackend<'source, S> {
	view: NativeView<'source, S>,
}

impl<'source, S> ApfsBackend<'source, S> {
	#[must_use]
	pub fn new(view: NativeView<'source, S>) -> Self {
		Self { view }
	}
}

impl<S: ReadAt> ApfsBackend<'_, S> {
	/// Checked metadata for this volume at the selected checkpoint.
	#[must_use]
	pub fn info(&self) -> VolumeInfo {
		VolumeInfo::from(self.view.volume())
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
			parent: None,
		})
	}

	fn numeric_id(&self, node: Node) -> Option<u64> {
		Some(node.inode.as_raw())
	}

	fn quarantine(
		&mut self,
		node: Node,
		scope: QuarantineScope,
	) -> Result<Quarantine, Self::Error> {
		self.check_quarantine(node, scope)
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
			.map(|entry| directory_entry(parent.inode, entry)))
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
			if !group
				.iter()
				.any(|link| link.sibling.as_raw() == sibling.0 && Some(link.parent) == node.parent)
			{
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
				.map(ApfsFork::Attribute),
			_ => return Err(AdapterError::ForkKind),
		};
		Ok(fork.map(|mut stream| read(&mut stream)))
	}

	fn list_xattrs(
		&mut self,
		node: Node,
		limit: NonZeroU32,
	) -> Result<Option<Vec<RawName>>, Self::Error> {
		self.attribute_names(node.inode, limit).map(Some)
	}

	fn with_xattr<T>(
		&mut self,
		node: Node,
		name: LookupName<'_>,
		read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> Result<Option<T>, Self::Error> {
		let LookupName::Utf8(name) = name else {
			return Err(AdapterError::NameEncoding);
		};
		Ok(self
			.view
			.open_xattr(node.inode, name)?
			.map(|value| read(&mut ApfsFork::Attribute(value))))
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
			entries.push(directory_entry(self.directory, entry));
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
	Attribute(XattrData<'view, 'source, S>),
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
			Self::Attribute(data) => data.len(),
		}
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Self::Error> {
		match self {
			Self::Data(data) => data.read_exact_at(ByteOffset(offset), output)?,
			Self::Attribute(data) => data.read_exact_at(ByteOffset(offset), output)?,
		}
		Ok(())
	}
}

fn directory_entry(parent: InodeId, entry: NativeEntry) -> DirectoryEntry<Node> {
	DirectoryEntry {
		name: RawName::Utf8(entry.name),
		node: Node {
			inode: entry.inode,
			sibling: entry.sibling,
			parent: Some(parent),
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
