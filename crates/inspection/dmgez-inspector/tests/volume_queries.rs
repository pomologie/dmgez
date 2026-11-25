use std::{
	cell::Cell,
	io,
	num::{NonZeroU16, NonZeroU32},
};

use dmgez_inspector::volume_queries::{
	Action, Error, InvalidPath, Kind, MAX_FORK_BYTES, MAX_LIST_LIMIT, MAX_LIST_OFFSET,
	MAX_TREE_DEPTH, MAX_TREE_LIMIT, Name, cat, list, stat, tree,
};
use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
		Metadata, NodeKind, RawName, ViewError, VolumeBackend, VolumeView,
	},
};
use serde_json::{json, to_value};

#[path = "volume_queries/search.rs"]
mod search;

const ROOT: u16 = 2;
const FOLDER: u16 = 10;
const PAYLOAD: u16 = 11;
const ALIAS: u16 = 12;
const LINK: u16 = 13;
const INVALID_UNICODE: u16 = 14;
const NESTED: u16 = 15;
const ESCAPED: u16 = 16;
const OWNER: u32 = 501;
const GROUP: u32 = 20;
const FILE_MODE: u16 = 0o100640;
const DIRECTORY_MODE: u16 = 0o040755;
const LINK_MODE: u16 = 0o120755;
const LINK_STORAGE_ID: u64 = (1 << 53) + 1;
const LINK_COUNT: NonZeroU32 = NonZeroU32::new(2).unwrap();
const LINK_PAYLOAD: &[u8] = b"/folder/\xff";
const RESOURCE: &[u8] = b"\0\xff\n";
const CAT_CHUNK_BYTES: usize = 64 * 1024;
static DATA: [u8; 2 * CAT_CHUNK_BYTES + 3] = [0x9c; 2 * CAT_CHUNK_BYTES + 3];
const ROOT_CHILDREN: &[u16] = &[FOLDER, PAYLOAD, ALIAS, LINK, INVALID_UNICODE, ESCAPED];
const UNPAIRED_SURROGATE: u16 = 0xd800;

#[derive(Default)]
struct Counts {
	roots: Cell<u32>,
	lookups: Cell<u32>,
	pages: Cell<u32>,
	open_fork: Cell<u32>,
	data_reads: Cell<u32>,
	link_reads: Cell<u32>,
}

struct Backend<'fixture> {
	counts: &'fixture Counts,
	stalled: bool,
	fail_at: Option<u64>,
	declared_fork_length: Option<u64>,
}

fn view(counts: &Counts) -> VolumeView<Backend<'_>> {
	VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			counts,
			stalled: false,
			fail_at: None,
			declared_fork_length: None,
		},
	)
}

struct Directory<'fixture> {
	counts: &'fixture Counts,
	remaining: &'static [u16],
	stalled: bool,
}

impl DirectoryReader for Directory<'_> {
	type Id = u16;
	type Error = io::Error;

	fn next_page(&mut self, limit: NonZeroU16) -> io::Result<DirectoryPage<u16>> {
		self.counts.pages.set(self.counts.pages.get() + 1);
		if self.stalled {
			return Ok(DirectoryPage {
				entries: Vec::new(),
				complete: false,
			});
		}
		let page_size = usize::from(limit.get());
		let count = self.remaining.len().min(page_size);
		let (head, tail) = self.remaining.split_at(count);
		self.remaining = tail;
		// Exercise engines that confirm an exact-fit page on their next scan.
		Ok(DirectoryPage {
			entries: head.iter().copied().map(entry).collect(),
			complete: count < page_size,
		})
	}
}

struct ReadFork<'fixture> {
	counts: &'fixture Counts,
	bytes: &'static [u8],
	fail_at: Option<u64>,
	declared_length: u64,
}

impl ForkReader for ReadFork<'_> {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.declared_length
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<()> {
		self.counts.data_reads.set(self.counts.data_reads.get() + 1);
		assert!(output.len() <= CAT_CHUNK_BYTES);
		if self.fail_at == Some(offset) {
			return Err(io::Error::other("injected read failure"));
		}
		let start = usize::try_from(offset).unwrap();
		output.copy_from_slice(&self.bytes[start..start + output.len()]);
		Ok(())
	}
}

impl VolumeBackend for Backend<'_> {
	type Id = u16;
	type Error = io::Error;
	type Directory<'view>
		= Directory<'view>
	where
		Self: 'view;

	fn root(&mut self) -> io::Result<u16> {
		self.counts.roots.set(self.counts.roots.get() + 1);
		Ok(ROOT)
	}

	fn numeric_id(&self, node: u16) -> Option<u64> {
		Some(node.into())
	}

	fn lookup(
		&mut self,
		parent: u16,
		name: LookupName<'_>,
	) -> io::Result<Option<DirectoryEntry<u16>>> {
		self.counts.lookups.set(self.counts.lookups.get() + 1);
		let LookupName::Utf8(name) = name else {
			return Ok(None);
		};
		Ok(children(parent).iter().copied().map(entry).find(
			|entry| matches!(&entry.name, RawName::Utf8(stored) if stored.eq_ignore_ascii_case(name)),
		))
	}

	fn stat(&mut self, node: u16) -> io::Result<Option<Metadata<u64>>> {
		let kind = node_kind(node);
		Ok(Some(Metadata {
			kind,
			mode: match kind {
				NodeKind::Directory => DIRECTORY_MODE,
				NodeKind::Symlink => LINK_MODE,
				_ => FILE_MODE,
			},
			uid: OWNER,
			gid: GROUP,
			hard_link: matches!(node, PAYLOAD | ALIAS).then_some(HardLink {
				storage: LINK_STORAGE_ID,
				count: LINK_COUNT,
			}),
		}))
	}

	fn read_dir(&mut self, directory: u16) -> io::Result<Self::Directory<'_>> {
		Ok(Directory {
			counts: self.counts,
			remaining: children(directory),
			stalled: self.stalled,
		})
	}

	fn with_fork<T>(
		&mut self,
		node: u16,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = io::Error>) -> T,
	) -> io::Result<Option<T>> {
		self.counts.open_fork.set(self.counts.open_fork.get() + 1);
		let bytes: &'static [u8] = match (node, kind) {
			(PAYLOAD | ALIAS, ForkKind::Data) => &DATA,
			(PAYLOAD | ALIAS, ForkKind::Resource) => RESOURCE,
			(NESTED, ForkKind::Data) => &[],
			_ => return Ok(None),
		};
		Ok(Some(read(&mut ReadFork {
			counts: self.counts,
			bytes,
			fail_at: self.fail_at,
			declared_length: self.declared_fork_length.unwrap_or(bytes.len() as u64),
		})))
	}

	fn read_link(&mut self, node: u16) -> io::Result<Option<Vec<u8>>> {
		self.counts.link_reads.set(self.counts.link_reads.get() + 1);
		Ok((node == LINK).then(|| LINK_PAYLOAD.to_vec()))
	}
}

fn children(parent: u16) -> &'static [u16] {
	match parent {
		ROOT => ROOT_CHILDREN,
		FOLDER => &[NESTED],
		_ => &[],
	}
}

fn node_kind(node: u16) -> NodeKind {
	match node {
		ROOT | FOLDER => NodeKind::Directory,
		LINK => NodeKind::Symlink,
		_ => NodeKind::RegularFile,
	}
}

fn entry(node: u16) -> DirectoryEntry<u16> {
	let name = match node {
		FOLDER => "folder",
		PAYLOAD => "payload",
		ALIAS => "alias",
		LINK => "link",
		NESTED => "nested",
		ESCAPED => "\u{1b}cmd",
		INVALID_UNICODE => {
			return DirectoryEntry {
				name: RawName::Utf16(vec![UNPAIRED_SURROGATE, u16::from(b'x')]),
				node,
				kind: node_kind(node),
			};
		}
		_ => unreachable!("fixture entry"),
	};
	DirectoryEntry {
		name: RawName::Utf8(name.into()),
		node,
		kind: node_kind(node),
	}
}

#[test]
fn ls_looks_ahead_and_preserves_raw_names() {
	let counts = Counts::default();
	let mut view = view(&counts);
	let first = list(&mut view, "/", 0, 3).unwrap();
	assert_eq!(first.next_offset, Some(3));
	assert_eq!(first.entries[0].name.to_string(), "folder");
	assert!(!first.truncated);
	let last = list(&mut view, "/", 3, 3).unwrap();
	assert!(last.next_offset.is_none());
	assert_eq!(last.entries[1].name.to_string(), r"\u{d800}x");
	assert_eq!(last.entries[2].name.to_string(), r"\u{1b}cmd");
	let serialized = to_value(&last).unwrap();
	assert_eq!(serialized["schema_version"], 1);
	assert_eq!(serialized["operation"], "ls");
	assert_eq!(serialized["volume"], json!({"kind":"whole_disk"}));
	assert_eq!(
		serialized["entries"][1]["name"],
		json!({"encoding": "utf16", "value": [UNPAIRED_SURROGATE, b'x']})
	);
	assert!(list(&mut view, "/", 7, 3).unwrap().entries.is_empty());
	assert_eq!(counts.open_fork.get(), 0);
}

#[test]
fn tree_honors_depth_total_and_native_sibling_order() {
	let counts = Counts::default();
	let mut view = view(&counts);
	let empty = tree(&mut view, "/", 0, 1).unwrap();
	assert!(empty.entries.is_empty() && !empty.truncated);
	assert_eq!(counts.pages.get(), 0);

	let shallow = tree(&mut view, "/", 1, ROOT_CHILDREN.len() as u32).unwrap();
	assert_eq!(shallow.entries.len(), ROOT_CHILDREN.len());
	assert!(!shallow.truncated);
	assert!(shallow.entries.iter().all(|item| item.depth == 1));
	let cap = tree(&mut view, "/", 2, ROOT_CHILDREN.len() as u32).unwrap();
	assert_eq!(cap.entries.len(), ROOT_CHILDREN.len());
	assert!(cap.truncated);
	let full = tree(&mut view, "/", 2, (ROOT_CHILDREN.len() + 1) as u32).unwrap();
	assert_eq!(full.entries[0].name.to_string(), "folder");
	assert_eq!(full.entries[1].name.to_string(), "nested");
	assert_eq!(full.entries[1].depth, 2);
	assert_eq!(full.entries[2].name.to_string(), "payload");
	assert!(!full.truncated);
	assert_eq!(counts.link_reads.get(), 0);
	assert_eq!(counts.open_fork.get(), 0);
}

#[test]
fn stat_reports_real_spelling_and_native_identities_without_opening_forks() {
	let counts = Counts::default();
	let mut view = view(&counts);
	let file = stat(&mut view, "/PAYLOAD").unwrap();
	assert_eq!(file.name, Some(Name::Utf8("payload".into())));
	assert_eq!(file.native_id.unwrap().0, u64::from(PAYLOAD));
	assert_eq!(file.mode, FILE_MODE);
	assert_eq!(file.hard_link_count, Some(LINK_COUNT.get()));
	assert_eq!(file.hard_link_storage_id.unwrap().0, LINK_STORAGE_ID);
	assert_eq!(file.uid, OWNER);
	assert_eq!(file.gid, GROUP);
	assert!(file.link_target.is_none());
	let alias = to_value(stat(&mut view, "/alias").unwrap()).unwrap();
	assert_eq!(alias["native_id"], ALIAS.to_string());
	assert_eq!(alias["hard_link_storage_id"], LINK_STORAGE_ID.to_string());
	assert_eq!(counts.open_fork.get(), 0);
	assert_eq!(counts.data_reads.get(), 0);
	assert_eq!(counts.link_reads.get(), 0);
	let link = stat(&mut view, "/link").unwrap();
	assert_eq!(link.kind, Kind::Symlink);
	assert_eq!(link.link_target.as_deref(), Some(LINK_PAYLOAD));
	assert_eq!(counts.link_reads.get(), 1);
}

#[test]
fn rejected_queries_do_not_read_through_links_or_backtrack() {
	let counts = Counts::default();
	let mut view = view(&counts);
	for path in [
		"relative",
		"//payload",
		"/folder/../payload",
		"/folder/./nested",
		"/bad\0name",
	] {
		assert!(matches!(stat(&mut view, path), Err(Error::InvalidPath(_))));
	}
	assert!(matches!(
		stat(&mut view, &format!("/{}", "a".repeat(1025))),
		Err(Error::InvalidPath(InvalidPath::ComponentTooLong { .. }))
	));
	assert!(matches!(
		list(&mut view, "/", MAX_LIST_OFFSET + 1, 1),
		Err(Error::ListOffset { .. })
	));
	assert!(matches!(
		list(&mut view, "/", 0, MAX_LIST_LIMIT + 1),
		Err(Error::ListLimit { .. })
	));
	assert!(matches!(
		tree(&mut view, "/", MAX_TREE_DEPTH + 1, 1),
		Err(Error::TreeDepth { .. })
	));
	assert!(matches!(
		tree(&mut view, "/", 1, MAX_TREE_LIMIT + 1),
		Err(Error::TreeLimit { .. })
	));
	assert_eq!(counts.roots.get(), 0);
	assert!(matches!(
		stat(&mut view, "/link/nested"),
		Err(Error::View {
			action: Action::Lookup { component: 2 },
			source: ViewError::WrongKind {
				actual: NodeKind::Symlink,
				..
			},
		})
	));
	assert_eq!(counts.lookups.get(), 1);
	assert_eq!(counts.link_reads.get(), 0);
	assert!(matches!(
		tree(&mut view, "/payload", 0, 1),
		Err(Error::View {
			source: ViewError::WrongKind { .. },
			..
		})
	));
}

#[test]
fn directory_scan_fails_on_empty_incomplete_page() {
	let counts = Counts::default();
	let mut view = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			counts: &counts,
			stalled: true,
			fail_at: None,
			declared_fork_length: None,
		},
	);
	assert!(matches!(
		list(&mut view, "/", 0, 1),
		Err(Error::StalledDirectory)
	));
	assert_eq!(counts.pages.get(), 1);
	assert!(matches!(
		tree(&mut view, "/", 1, 1),
		Err(Error::StalledDirectory)
	));
	assert_eq!(counts.pages.get(), 2);
}

#[test]
fn cat_streams_exact_data_and_distinguishes_absent_and_empty_forks() {
	let counts = Counts::default();
	let mut view = view(&counts);
	let mut output = Vec::new();
	assert_eq!(
		cat(&mut view, "/alias", ForkKind::Data, &mut output).unwrap(),
		DATA.len() as u64
	);
	assert_eq!(output, DATA);
	assert_eq!(counts.data_reads.get(), 3);
	output.clear();
	assert_eq!(
		cat(&mut view, "/payload", ForkKind::Resource, &mut output).unwrap(),
		RESOURCE.len() as u64
	);
	assert_eq!(output, RESOURCE);
	output.clear();
	assert_eq!(
		cat(&mut view, "/folder/nested", ForkKind::Data, &mut output).unwrap(),
		0
	);
	assert!(output.is_empty());
	assert!(matches!(
		cat(&mut view, "/folder/nested", ForkKind::Resource, &mut output),
		Err(Error::ForkAbsent {
			kind: ForkKind::Resource
		})
	));
	assert!(matches!(
		cat(&mut view, "/link", ForkKind::Data, &mut output),
		Err(Error::View {
			source: ViewError::WrongKind {
				actual: NodeKind::Symlink,
				..
			},
			..
		})
	));
}

#[test]
fn cat_refuses_oversized_forks_before_writing_and_reports_late_errors() {
	let counts = Counts::default();
	let mut output = Vec::new();
	let mut oversized = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			counts: &counts,
			stalled: false,
			fail_at: None,
			declared_fork_length: Some(MAX_FORK_BYTES + 1),
		},
	);
	assert!(matches!(
		cat(&mut oversized, "/payload", ForkKind::Data, &mut output),
		Err(Error::ForkLimit {
			maximum: MAX_FORK_BYTES,
			..
		})
	));
	assert!(output.is_empty());
	assert_eq!(counts.data_reads.get(), 0);

	let mut broken = VolumeView::new(
		VolumeId::WholeDisk,
		Backend {
			counts: &counts,
			stalled: false,
			fail_at: Some(CAT_CHUNK_BYTES as u64),
			declared_fork_length: None,
		},
	);
	assert!(matches!(
		cat(&mut broken, "/payload", ForkKind::Data, &mut output),
		Err(Error::View { action: Action::ReadFork { offset }, .. })
			if offset == CAT_CHUNK_BYTES as u64
	));
	assert_eq!(output, DATA[..CAT_CHUNK_BYTES]);

	let mut short_output = &mut [0u8; 1][..];
	assert!(matches!(
		cat(&mut broken, "/payload", ForkKind::Resource, &mut short_output),
		Err(Error::Output(error)) if error.kind() == io::ErrorKind::WriteZero
	));
}
