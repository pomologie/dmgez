use std::{collections::BTreeMap, io, num::NonZeroU16};

use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
		Metadata, NodeKind, RawName, VolumeBackend, VolumeView,
	},
};

use super::{ChangeKind, Error, Field, Limits, Outcome, compare};

const ROOT: u32 = 1;
const USER_ID: u32 = 501;
const GROUP_ID: u32 = 20;
const ROOT_MODE: u16 = 0o40755;
const FILE_MODE: u16 = 0o100644;
const LINK_MODE: u16 = 0o120777;
const SHARED_BYTE_CONTENT: &[u8] = b"test data";

#[test]
fn compares_group_paths_independently_of_native_object_and_storage_ids() {
	let mut left = view([
		("z", TestNode::file(8).linked(100, 2)),
		("a", TestNode::file(2).linked(100, 2)),
	]);
	let mut right = view([
		("a", TestNode::file(90).linked(200, 2)),
		("z", TestNode::file(120).linked(200, 2)),
	]);
	let report = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert!(report.changes.is_empty());
	assert_eq!(report.left_entries, 3);
	assert_eq!(report.right_entries, 3);

	let mut right = view([
		("a", TestNode::file(90).linked(200, 2)),
		("b", TestNode::file(120).linked(200, 2)),
	]);
	let report = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	let changes: Vec<_> = report
		.changes
		.iter()
		.map(|change| {
			(
				change.path[0].as_str(),
				change.kind,
				change.fields.as_slice(),
			)
		})
		.collect();
	assert_eq!(
		changes,
		[
			("a", ChangeKind::Changed, &[Field::HardLinkTopology][..]),
			("b", ChangeKind::Added, &[]),
			("z", ChangeKind::Removed, &[]),
		]
	);
}

#[test]
fn singleton_link_storage_is_ordinary_one_name_topology() {
	let mut left = view([("file", TestNode::file(2).linked(5, 1))]);
	let mut right = view([("file", TestNode::file(2))]);
	let report = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
}

#[test]
fn preserves_exact_comparison_scope_and_distinguishes_empty_forks() {
	let mut changed = TestNode::file(2);
	changed.mode = FILE_MODE | 0o100;
	changed.resource = Some(b"");
	let mut left = view([("file", TestNode::file(2))]);
	let mut right = view([("file", changed)]);
	let report = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(report.changes[0].fields, [Field::Mode, Field::ResourceFork]);
	assert_eq!(report.compared_bytes, SHARED_BYTE_CONTENT.len() as u64);

	let json = serde_json::to_value(report).unwrap();
	assert_eq!(json["changes"][0]["path"], serde_json::json!(["file"]));
	assert_eq!(json["policy"]["paths"], "exact_stored_text");
	assert_eq!(json["policy"]["contents"], "exact_bytes");
	assert!(
		json["policy"]["excluded"]
			.as_array()
			.unwrap()
			.contains(&serde_json::json!("timestamps"))
	);
	assert!(json["compared_bytes"].is_string());
}

#[test]
fn compares_and_charges_exact_non_unicode_link_targets() {
	const LEFT_TARGET: &[u8] = b"/\xffleft";
	const RIGHT_TARGET: &[u8] = b"/\xffLEFT";
	let mut left = view([("symbolic link", TestNode::link(2, LEFT_TARGET))]);
	let mut right = view([("symbolic link", TestNode::link(3, RIGHT_TARGET))]);
	let complete = Limits {
		max_bytes: LEFT_TARGET.len() as u64,
		..Limits::default()
	};
	let report = compare(&mut left, &mut right, complete, || false).unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(report.changes[0].fields, [Field::SymlinkTarget]);
	assert_eq!(report.compared_bytes, complete.max_bytes);

	let incomplete = Limits {
		max_bytes: complete.max_bytes - 1,
		..complete
	};
	assert!(matches!(
		compare(&mut left, &mut right, incomplete, || false),
		Err(Error::ByteLimit { limit }) if limit == incomplete.max_bytes
	));
}

#[test]
fn identical_forks_require_every_byte_under_the_budget() {
	let mut left = view([("file", TestNode::file(2))]);
	let mut right = view([("file", TestNode::file(3))]);
	let limits = Limits {
		max_bytes: SHARED_BYTE_CONTENT.len() as u64 - 1,
		..Limits::default()
	};
	assert!(matches!(
		compare(&mut left, &mut right, limits, || false),
		Err(Error::ByteLimit { limit }) if limit == limits.max_bytes
	));
}

struct TestNode {
	id: u32,
	kind: NodeKind,
	mode: u16,
	data: Option<&'static [u8]>,
	resource: Option<&'static [u8]>,
	hard_link: Option<HardLink<u64>>,
	target: Option<&'static [u8]>,
}

impl TestNode {
	fn file(id: u32) -> Self {
		Self {
			id,
			kind: NodeKind::RegularFile,
			mode: FILE_MODE,
			data: Some(SHARED_BYTE_CONTENT),
			resource: None,
			hard_link: None,
			target: None,
		}
	}

	fn link(id: u32, target: &'static [u8]) -> Self {
		Self {
			kind: NodeKind::Symlink,
			mode: LINK_MODE,
			data: None,
			target: Some(target),
			..Self::file(id)
		}
	}

	fn linked(mut self, storage: u64, count: u32) -> Self {
		self.hard_link = Some(HardLink {
			storage,
			count: count.try_into().unwrap(),
		});
		self
	}
}

fn view<const N: usize>(children: [(&str, TestNode); N]) -> VolumeView<TestTree> {
	let mut entries = Vec::new();
	let mut nodes = BTreeMap::new();
	for (name, node) in children {
		entries.push(DirectoryEntry {
			name: RawName::Utf8(name.to_owned()),
			node: node.id,
			kind: node.kind,
		});
		nodes.insert(node.id, node);
	}
	VolumeView::new(VolumeId::WholeDisk, TestTree { entries, nodes })
}

struct TestTree {
	entries: Vec<DirectoryEntry<u32>>,
	nodes: BTreeMap<u32, TestNode>,
}

impl VolumeBackend for TestTree {
	type Id = u32;
	type Error = io::Error;
	type Directory<'a> = Page<'a>;

	fn root(&mut self) -> io::Result<u32> {
		Ok(ROOT)
	}

	fn lookup(
		&mut self,
		_parent: u32,
		_name: LookupName<'_>,
	) -> io::Result<Option<DirectoryEntry<u32>>> {
		panic!("semantic comparison uses stored directory pages")
	}

	fn stat(&mut self, node: u32) -> io::Result<Option<Metadata<u64>>> {
		if node == ROOT {
			return Ok(Some(Metadata {
				kind: NodeKind::Directory,
				mode: ROOT_MODE,
				uid: USER_ID,
				gid: GROUP_ID,
				hard_link: None,
			}));
		}
		Ok(self.nodes.get(&node).map(|node| Metadata {
			kind: node.kind,
			mode: node.mode,
			uid: USER_ID,
			gid: GROUP_ID,
			hard_link: node.hard_link.clone(),
		}))
	}

	fn read_dir(&mut self, directory: u32) -> io::Result<Page<'_>> {
		assert_eq!(directory, ROOT);
		Ok(Page(&self.entries))
	}

	fn with_fork<T>(
		&mut self,
		node: u32,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> io::Result<Option<T>> {
		let node = &self.nodes[&node];
		let fork = match kind {
			ForkKind::Data => node.data,
			ForkKind::Resource => node.resource,
			_ => panic!("unrecognized test fork"),
		};
		Ok(fork.map(|bytes| read(&mut Fork(bytes))))
	}

	fn read_link(&mut self, node: u32) -> io::Result<Option<Vec<u8>>> {
		Ok(self.nodes[&node].target.map(<[u8]>::to_vec))
	}
}

struct Page<'a>(&'a [DirectoryEntry<u32>]);

impl DirectoryReader for Page<'_> {
	type Id = u32;
	type Error = io::Error;

	fn next_page(&mut self, limit: NonZeroU16) -> io::Result<DirectoryPage<u32>> {
		let count = self.0.len().min(usize::from(limit.get()));
		let (page, rest) = self.0.split_at(count);
		self.0 = rest;
		Ok(DirectoryPage {
			entries: page.to_vec(),
			complete: rest.is_empty(),
		})
	}
}

struct Fork(&'static [u8]);

impl ForkReader for Fork {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.0.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<()> {
		let start = offset as usize;
		output.copy_from_slice(&self.0[start..start + output.len()]);
		Ok(())
	}
}
