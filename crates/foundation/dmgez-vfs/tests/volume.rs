//! C5 read-view contract coverage for D-READ-03.

use std::{
	cell::Cell,
	error::Error,
	num::{NonZeroU16, NonZeroU32},
};

use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, FINDER_INFO_BYTES, FinderInfo, ForkKind,
		ForkReader, HardLink, LookupName, Metadata, NodeKind, RawName, ViewError, VolumeBackend,
		VolumeView,
	},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Id {
	Root,
	File,
	Alias,
	Link,
}

const UNPAIRED_SURROGATE: u16 = 0xd800;
const STORED_NAME: &[u16] = &[UNPAIRED_SURROGATE, b'a' as u16];
const ALIAS: &str = "alias";
const LINK: &str = "link";
const LINK_PAYLOAD: &[u8] = b"../file\xff";
const DATA: &[u8] = b"content";
const DATA_ATTRIBUTE: &str = "org.pomologie.data";
const EMPTY_ATTRIBUTE: &str = "org.pomologie.empty";
const ZERO_FINDER_INFO: [u8; FINDER_INFO_BYTES] = [0; FINDER_INFO_BYTES];
const MODE: u16 = 0o100640;
const OWNER: u32 = 501;
const GROUP: u32 = 20;
const STORAGE: u64 = 11;
const LINK_COUNT: NonZeroU32 = NonZeroU32::new(2).unwrap();
const PRIVATE_MESSAGE: &str = "engine detail /private/folder";
const PAGE_LIMIT: NonZeroU16 = NonZeroU16::new(2).unwrap();

#[derive(Debug, thiserror::Error)]
#[error("{PRIVATE_MESSAGE}")]
struct EngineError;

struct Backend<'fixture> {
	reads: &'fixture Cell<usize>,
	position: usize,
}

struct ReadFork<'backend> {
	data: &'static [u8],
	reads: &'backend Cell<usize>,
	position: &'backend mut usize,
}

impl ForkReader for ReadFork<'_> {
	type Error = EngineError;

	fn len(&self) -> u64 {
		self.data.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Self::Error> {
		self.reads.set(self.reads.get() + 1);
		let start = usize::try_from(offset).unwrap();
		*self.position = start + output.len();
		output.copy_from_slice(&self.data[start..*self.position]);
		Ok(())
	}
}

struct Directory {
	next: usize,
}

impl DirectoryReader for Directory {
	type Id = Id;
	type Error = EngineError;

	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<Id>, EngineError> {
		let children = [Id::File, Id::Alias, Id::Link];
		let entries: Vec<_> = children
			.into_iter()
			.skip(self.next)
			.take(usize::from(limit.get()))
			.map(entry)
			.collect();
		self.next += entries.len();
		Ok(DirectoryPage {
			entries,
			complete: self.next == children.len(),
		})
	}
}

impl VolumeBackend for Backend<'_> {
	type Id = Id;
	type Error = EngineError;
	type Directory<'view>
		= Directory
	where
		Self: 'view;

	fn root(&mut self) -> Result<Id, EngineError> {
		Ok(Id::Root)
	}

	fn lookup(
		&mut self,
		parent: Id,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<Id>>, EngineError> {
		assert_eq!(parent, Id::Root);
		let child = match name {
			LookupName::StoredUtf16(STORED_NAME) => Id::File,
			LookupName::Utf8(ALIAS) => Id::Alias,
			LookupName::Utf8(LINK) => Id::Link,
			_ => return Ok(None),
		};
		Ok(Some(entry(child)))
	}

	fn stat(&mut self, node: Id) -> Result<Option<Metadata<u64>>, EngineError> {
		let (kind, hard_link) = match node {
			Id::Root => (NodeKind::Directory, None),
			Id::Link => (NodeKind::Symlink, None),
			Id::File | Id::Alias => (
				NodeKind::RegularFile,
				Some(HardLink {
					storage: STORAGE,
					count: LINK_COUNT,
				}),
			),
		};
		Ok(Some(Metadata {
			kind,
			mode: MODE,
			uid: OWNER,
			gid: GROUP,
			hard_link,
		}))
	}

	fn finder_info(&mut self, node: Id) -> Result<FinderInfo, EngineError> {
		Ok(match node {
			Id::Root => FinderInfo::Absent,
			Id::File | Id::Alias => FinderInfo::Present(ZERO_FINDER_INFO),
			Id::Link => FinderInfo::Unavailable,
		})
	}

	fn read_dir(&mut self, directory: Id) -> Result<Self::Directory<'_>, EngineError> {
		assert_eq!(directory, Id::Root);
		Ok(Directory { next: 0 })
	}

	fn list_xattrs(
		&mut self,
		node: Id,
		_limit: NonZeroU32,
	) -> Result<Option<Vec<RawName>>, EngineError> {
		Ok(match node {
			Id::File => Some(vec![
				RawName::Utf8(DATA_ATTRIBUTE.into()),
				RawName::Utf8(EMPTY_ATTRIBUTE.into()),
			]),
			Id::Root | Id::Alias => Some(Vec::new()),
			Id::Link => None,
		})
	}

	fn with_xattr<T>(
		&mut self,
		node: Id,
		name: LookupName<'_>,
		read: impl FnOnce(&mut dyn ForkReader<Error = EngineError>) -> T,
	) -> Result<Option<T>, EngineError> {
		let data = match (node, name) {
			(Id::File, LookupName::Utf8(DATA_ATTRIBUTE)) => DATA,
			(Id::File, LookupName::Utf8(EMPTY_ATTRIBUTE)) => &[],
			_ => return Ok(None),
		};
		Ok(Some(read(&mut ReadFork {
			data,
			reads: self.reads,
			position: &mut self.position,
		})))
	}

	fn with_fork<T>(
		&mut self,
		node: Id,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = EngineError>) -> T,
	) -> Result<Option<T>, EngineError> {
		let data = match (node, kind) {
			(Id::File | Id::Alias, ForkKind::Data) => DATA,
			(Id::Alias, ForkKind::Resource) => b"",
			(Id::File, ForkKind::Resource) => return Ok(None),
			_ => return Err(EngineError),
		};
		Ok(Some(read(&mut ReadFork {
			data,
			reads: self.reads,
			position: &mut self.position,
		})))
	}

	fn read_link(&mut self, node: Id) -> Result<Option<Vec<u8>>, EngineError> {
		assert_eq!(node, Id::Link);
		Ok(Some(LINK_PAYLOAD.to_vec()))
	}
}

#[test]
fn xattrs_preserve_session_limits_and_empty_values() {
	let reads = Cell::new(0);
	let mut first = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let mut second = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let root = first.root().unwrap();
	let file = first
		.lookup(&root, LookupName::StoredUtf16(STORED_NAME))
		.unwrap()
		.unwrap();
	let link = first
		.lookup(&root, LookupName::Utf8(LINK))
		.unwrap()
		.unwrap();
	assert!(matches!(
		first.list_xattrs(&file.node, NonZeroU32::MIN),
		Err(ViewError::AttributeLimit {
			limit: 1,
			entries: 2
		})
	));
	assert!(matches!(
		second.list_xattrs(&file.node, LINK_COUNT),
		Err(ViewError::ForeignHandle)
	));
	assert_eq!(
		first.list_xattrs(&root, LINK_COUNT).unwrap(),
		Some(Vec::new())
	);
	assert_eq!(first.list_xattrs(&link.node, LINK_COUNT).unwrap(), None);
	let names = first.list_xattrs(&file.node, LINK_COUNT).unwrap().unwrap();
	assert!(matches!(
		second.with_xattr(&file.node, names[0].as_lookup(), |_| panic!(
			"foreign attribute"
		)),
		Err(ViewError::ForeignHandle)
	));
	assert!(matches!(
		first
			.with_xattr(&file.node, names[0].as_lookup(), |value| {
				value.read_exact_at(value.len(), &mut [0])
			})
			.unwrap(),
		Some(Err(ViewError::OutOfRange { .. }))
	));
	assert_eq!(
		first
			.with_xattr(&file.node, names[1].as_lookup(), |value| value.is_empty())
			.unwrap(),
		Some(true)
	);
	assert_eq!(
		first
			.with_xattr(&root, names[0].as_lookup(), |_| ())
			.unwrap(),
		None
	);
	assert_eq!(reads.get(), 0);
}

fn entry(node: Id) -> DirectoryEntry<Id> {
	let (name, kind) = match node {
		Id::File => (RawName::Utf16(STORED_NAME.to_vec()), NodeKind::RegularFile),
		Id::Alias => (RawName::Utf8(ALIAS.to_owned()), NodeKind::RegularFile),
		Id::Link => (RawName::Utf8(LINK.to_owned()), NodeKind::Symlink),
		Id::Root => unreachable!("root is a directory parent"),
	};
	DirectoryEntry { name, node, kind }
}

#[test]
fn stored_names_and_visible_handles_survive_bounded_directory_pages() {
	let reads = Cell::new(0);
	let mut view = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let root = view.root().unwrap();
	assert_eq!(root, view.root().unwrap());
	assert_eq!(view.partition(), VolumeId::WholeDisk);
	let mut scan = view.read_dir(&root).unwrap();
	let first = scan.next_page(PAGE_LIMIT).unwrap();
	assert!(!first.complete);
	assert_eq!(first.entries.len(), usize::from(PAGE_LIMIT.get()));
	let second = scan.next_page(PAGE_LIMIT).unwrap();
	assert_eq!(second.entries.len(), 1);
	assert!(second.complete);
	let end = scan.next_page(NonZeroU16::MIN).unwrap();
	assert!(end.complete);
	assert!(end.entries.is_empty());

	let mut children = first.entries.into_iter().chain(second.entries);
	let file = children.next().unwrap();
	assert_eq!(file.name, RawName::Utf16(STORED_NAME.to_vec()));
	assert_eq!(
		view.lookup(&root, file.name.as_lookup()).unwrap(),
		Some(file.clone())
	);
	assert!(
		view.lookup(&root, LookupName::Utf8("missing"))
			.unwrap()
			.is_none()
	);
	let alias = children.next().unwrap();
	assert_ne!(file.node, alias.node);
	let metadata = view.stat(&file.node).unwrap();
	assert_eq!(
		(metadata.mode, metadata.uid, metadata.gid),
		(MODE, OWNER, GROUP)
	);
	assert_eq!(
		metadata.hard_link,
		view.stat(&alias.node).unwrap().hard_link
	);

	let mut other = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let other_root = other.root().unwrap();
	assert_ne!(root, other_root);
	assert!(matches!(
		other.stat(&file.node),
		Err(ViewError::ForeignHandle)
	));
	let other_file = other
		.lookup(&other_root, file.name.as_lookup())
		.unwrap()
		.unwrap();
	assert_ne!(
		metadata.hard_link,
		other.stat(&other_file.node).unwrap().hard_link
	);
}

#[test]
fn exact_forks_distinguish_absent_empty_and_symlink_bytes() {
	let reads = Cell::new(0);
	let mut view = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let root = view.root().unwrap();
	let file = view
		.lookup(&root, LookupName::StoredUtf16(STORED_NAME))
		.unwrap()
		.unwrap()
		.node;
	let alias = view
		.lookup(&root, LookupName::Utf8(ALIAS))
		.unwrap()
		.unwrap()
		.node;
	let symlink = view
		.lookup(&root, LookupName::Utf8(LINK))
		.unwrap()
		.unwrap()
		.node;

	view.with_fork(&file, ForkKind::Data, |fork| {
		let mut bytes = [0; DATA.len()];
		assert_eq!(fork.len(), DATA.len() as u64);
		fork.read_exact_at(0, &mut bytes).unwrap();
		assert_eq!(bytes, DATA);
		fork.read_exact_at(fork.len(), &mut []).unwrap();
		assert!(matches!(
			fork.read_exact_at(fork.len(), &mut bytes),
			Err(ViewError::OutOfRange { .. })
		));
		assert!(matches!(
			fork.read_exact_at(u64::MAX, &mut bytes),
			Err(ViewError::OutOfRange { .. })
		));
		assert_eq!(bytes, DATA);
	})
	.unwrap()
	.unwrap();
	assert_eq!(reads.get(), 2);
	assert_eq!(
		view.with_fork(&file, ForkKind::Resource, |_| panic!("missing resource"))
			.unwrap(),
		None::<()>
	);
	assert_eq!(
		view.with_fork(&alias, ForkKind::Resource, |fork| fork.is_empty())
			.unwrap(),
		Some(true)
	);

	assert_eq!(view.read_link(&symlink).unwrap(), LINK_PAYLOAD);
	assert!(matches!(
		view.with_fork(&symlink, ForkKind::Data, |_| panic!(
			"symbolic link opened as a file"
		)),
		Err(ViewError::WrongKind {
			actual: NodeKind::Symlink,
			..
		})
	));
}

#[test]
fn finder_info_retains_admitted_states_and_rejects_foreign_handles() {
	let reads = Cell::new(0);
	let mut view = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let mut other = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	let root = view.root().unwrap();
	assert_eq!(view.finder_info(&root).unwrap(), FinderInfo::Absent);
	assert!(matches!(
		other.finder_info(&root),
		Err(ViewError::ForeignHandle)
	));
	for name in [
		LookupName::StoredUtf16(STORED_NAME),
		LookupName::Utf8(ALIAS),
	] {
		let child = view.lookup(&root, name).unwrap().unwrap();
		assert_eq!(
			view.finder_info(&child.node).unwrap(),
			FinderInfo::Present(ZERO_FINDER_INFO)
		);
	}
	let link = view.lookup(&root, LookupName::Utf8(LINK)).unwrap().unwrap();
	assert_eq!(
		view.finder_info(&link.node).unwrap(),
		FinderInfo::Unavailable
	);
	assert_eq!(reads.get(), 0);
}

#[test]
fn native_name_resolution_is_unavailable_by_default() {
	let reads = Cell::new(0);
	let view = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			reads: &reads,
			position: 0,
		},
	);
	assert_eq!(view.native_name_profile(), None);
	assert_eq!(view.native_name_key(LookupName::Utf8(ALIAS)).unwrap(), None);
}

#[test]
fn public_backend_error_keeps_typed_cause_without_using_it_as_display() {
	let error = ViewError::Backend(EngineError);
	assert!(!error.to_string().contains(PRIVATE_MESSAGE));
	assert!(error.source().unwrap().is::<EngineError>());
}
