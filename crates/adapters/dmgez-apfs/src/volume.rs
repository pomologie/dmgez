use std::{
	error::Error as StdError,
	num::{NonZeroU16, NonZeroU32},
};

use apfs_core::{ByteOffset, ReadAt};
use apfs_format::object::ObjectId;
use apfs_reader::{
	DirectoryCursor, DirectoryEntry as NativeEntry, Error as NativeError, FileData, FileKind,
	InodeId, NameError, NameProfile, VolumeView as NativeView, XattrData,
};
use dmgez_vfs::volume::{
	DirectoryEntry, DirectoryPage, DirectoryReader, FinderInfo, ForkKind, ForkReader, HardLink,
	LookupName, Metadata, NativeNameKey, NativeNameProfile, NodeKind, Quarantine, QuarantineScope,
	RawName, VolumeBackend,
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
	#[error("could not derive an APFS native name key")]
	NameKey(#[from] NameError),
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

	fn native_name_profile(&self) -> Option<NativeNameProfile> {
		match self.view.name_profile() {
			NameProfile::Binary => Some(NativeNameProfile::ApfsBinary),
			NameProfile::Unicode9Normalized => Some(NativeNameProfile::ApfsUnicode9Normalized),
			NameProfile::Unicode9Folded => Some(NativeNameProfile::ApfsUnicode9Folded),
			_ => None,
		}
	}

	fn native_name_key(&self, name: LookupName<'_>) -> Result<Option<NativeNameKey>, Self::Error> {
		if self.native_name_profile().is_none() {
			return Ok(None);
		}
		let LookupName::Utf8(name) = name else {
			return Err(AdapterError::NameEncoding);
		};
		let bytes = self.view.name_profile().key(name)?;
		Ok(Some(NativeNameKey::new(bytes)))
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
		Ok(self
			.view
			.stat_entry(node.inode, node.parent, node.sibling)?
			.map(|entry| Metadata {
				kind: node_kind(entry.inode.kind),
				mode: entry.inode.mode,
				uid: entry.inode.owner,
				gid: entry.inode.group,
				hard_link: entry.hard_link_count.map(|count| HardLink {
					storage: entry.inode.id.as_raw(),
					count,
				}),
			}))
	}

	fn finder_info(&mut self, node: Node) -> Result<FinderInfo, Self::Error> {
		Ok(self
			.view
			.finder_info(node.inode)?
			.map_or(FinderInfo::Absent, FinderInfo::Present))
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
		let page = self
			.view
			.read_checked_dir(self.directory, self.cursor, limit)?;
		let mut entries = Vec::new();
		entries
			.try_reserve_exact(page.entries.len())
			.map_err(|_| AdapterError::Allocation)?;
		entries.extend(
			page.entries
				.into_iter()
				.map(|entry| directory_entry(self.directory, entry)),
		);
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
