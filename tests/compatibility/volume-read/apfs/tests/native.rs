//! D-READ-03 compatibility with independently generated native APFS records.

use std::{error::Error as StdError, num::NonZeroU16};

use apfs_container::{Container, LookupOptions};
use apfs_core::{ByteOffset, ReadAt};
use apfs_format::object::{ObjectId, TransactionId};
use apfs_reader::{Error as NativeError, Limit, TreeLimits, VolumeView as NativeView};
use dmgez_apfs_volume_read_compatibility::{AdapterError, ApfsBackend};
use dmgez_operations::extract::{
	NameError, NameLimits, NamePlanError, NameProfile, preflight_names,
};
use dmgez_vfs::volume::{
	ForkKind, LookupName, NodeHandle, NodeKind, RawName, ViewError, VolumeBackend, VolumeView,
};

#[path = "native/fixture.rs"]
mod fixture;
use fixture::{FixtureError, NATIVE_BLOCK_BYTES, NativeBlocks, VOLUME, disk, open_container};

const HARD_LINKS: &[u8] = include_bytes!("fixtures/sonoma-hard-link-blocks.bin");
const PAYLOADS: &[u8] = include_bytes!("fixtures/sonoma-payload-blocks.bin");
const APFS_VOLUME: ObjectId = ObjectId(1026);
const SELECTED_XID: TransactionId = TransactionId(4);
const FIRST_PAGE: NonZeroU16 = NonZeroU16::MIN;
const OVERSIZED_PAGE: NonZeroU16 = NonZeroU16::new(4).unwrap();
const EXPECTED_ROOT: [&str; 3] = ["Docs", "More", "primary.bin"];
const NATIVE_ALIAS: &str = "alias.bin";
const NATIVE_UNICODE_ALIAS: &str = "Écho.bin";
const MODE: u16 = 0o100644;
const DATA_PATTERN: &[u8] = b"APFS shared data fork\n";
const DATA_REPETITIONS: usize = 250;
const RESOURCE_PATTERN: &[u8] = b"APFS shared resource fork\n";
const RESOURCE_REPETITIONS: usize = 200;
const BOUNDARY_OFFSET: u64 = NATIVE_BLOCK_BYTES - 5;
const BOUNDARY_BYTES: usize = 29;
const LINK_NAME: &str = "link";
const RAW_TARGET: &[u8] = b"../large.bin";
const TARGET_NAME: &str = "large.bin";
const EMPTY_NAME: &str = "empty.txt";
const UNCAPTURED_BLOCK: u64 = 100;

#[test]
fn native_aliases_keep_distinct_handles_and_checked_shared_forks() {
	let blocks = NativeBlocks::from_packed(HARD_LINKS);
	let encoded = blocks.wrap_udif();
	let mut disk = disk(&encoded);
	let container = open_container(&mut disk, &blocks);
	let limits = TreeLimits {
		max_entries_per_page: EXPECTED_ROOT.len() as u16,
		..TreeLimits::default()
	};
	let mut view = open_view(&container, limits);
	let root = view.root().unwrap();
	let mut scan = view.read_dir(&root).unwrap();
	let mut root_entries = Vec::new();
	for name in EXPECTED_ROOT {
		assert!(matches!(
			scan.next_page(OVERSIZED_PAGE),
			Err(ViewError::Backend(AdapterError::Reader(
				NativeError::Limit(Limit::Page)
			)))
		));
		let page = scan.next_page(FIRST_PAGE).unwrap();
		assert_eq!(page.entries.len(), 1);
		assert_eq!(page.entries[0].name, RawName::Utf8(name.to_owned()));
		assert_eq!(page.complete, name == EXPECTED_ROOT[2]);
		root_entries.extend(page.entries);
	}
	assert!(scan.next_page(FIRST_PAGE).unwrap().complete);
	assert!(scan.next_page(FIRST_PAGE).unwrap().entries.is_empty());

	for entry in &root_entries {
		assert_eq!(
			view.lookup(&root, entry.name.as_lookup()).unwrap().as_ref(),
			Some(entry)
		);
	}
	let file = &root_entries[2].node;
	let alias = view
		.lookup(&root_entries[0].node, LookupName::Utf8(NATIVE_ALIAS))
		.unwrap()
		.unwrap();
	let unicode = view
		.lookup(
			&root_entries[1].node,
			LookupName::Utf8(NATIVE_UNICODE_ALIAS),
		)
		.unwrap()
		.unwrap();
	assert_ne!(file, &alias.node);
	assert_ne!(file, &unicode.node);
	assert_ne!(alias.node, unicode.node);
	assert_eq!(unicode.name, RawName::Utf8(NATIVE_UNICODE_ALIAS.to_owned()));

	let limits = NameLimits {
		page_entries: FIRST_PAGE,
		..NameLimits::default()
	};
	let unicode_parent = &root_entries[1].node;
	assert!(matches!(
		preflight_names(
			&mut view,
			unicode_parent,
			NameProfile::PortableAscii,
			limits,
			|| false
		),
		Err(NamePlanError::Name {
			reason: NameError::Encoding,
			..
		})
	));
	let plan = preflight_names(
		&mut view,
		unicode_parent,
		NameProfile::PosixUtf8,
		limits,
		|| false,
	)
	.unwrap();
	let planned_alias = plan
		.entries()
		.iter()
		.find(|entry| entry.name.as_str() == NATIVE_UNICODE_ALIAS)
		.unwrap();
	assert_eq!(planned_alias.kind, NodeKind::RegularFile);
	assert_eq!(planned_alias.source, unicode.node);

	let data = DATA_PATTERN.repeat(DATA_REPETITIONS);
	let resource = RESOURCE_PATTERN.repeat(RESOURCE_REPETITIONS);
	let fork = view.stat(file).unwrap();
	assert_eq!(fork.kind, NodeKind::RegularFile);
	assert_eq!(fork.mode, MODE);
	assert_eq!(fork.hard_link.as_ref().unwrap().count.get(), 3);
	for handle in [file, &alias.node, &unicode.node] {
		assert_eq!(view.stat(handle).unwrap(), fork);
		assert_fork(&mut view, handle, ForkKind::Data, &data);
		assert_fork(&mut view, handle, ForkKind::Resource, &resource);
	}

	let mut other = open_view(&container, TreeLimits::default());
	let other_root = other.root().unwrap();
	assert!(matches!(other.stat(file), Err(ViewError::ForeignHandle)));
	assert_ne!(root, other_root);
	let reopened = other
		.lookup(&other_root, LookupName::Utf8(EXPECTED_ROOT[2]))
		.unwrap()
		.unwrap();
	assert_ne!(
		other.stat(&reopened.node).unwrap().hard_link,
		fork.hard_link
	);
}

#[test]
fn native_symbolic_links_expose_only_target_bytes_and_empty_data_stays_present() {
	let blocks = NativeBlocks::from_packed(PAYLOADS);
	let encoded = blocks.wrap_udif();
	let mut disk = disk(&encoded);
	let container = open_container(&mut disk, &blocks);
	assert!(matches!(
		container
			.source()
			.read_exact_at(ByteOffset(UNCAPTURED_BLOCK * NATIVE_BLOCK_BYTES), &mut [0]),
		Err(FixtureError::MissingBlock(UNCAPTURED_BLOCK))
	));
	let mut view = open_view(&container, TreeLimits::default());
	let root = view.root().unwrap();
	let link = view
		.lookup(&root, LookupName::Utf8(LINK_NAME))
		.unwrap()
		.unwrap();
	assert_eq!(link.kind, NodeKind::Symlink);
	assert_eq!(view.read_link(&link.node).unwrap(), RAW_TARGET);
	assert!(matches!(
		view.with_fork(&link.node, ForkKind::Data, |_| ()),
		Err(ViewError::WrongKind {
			actual: NodeKind::Symlink,
			..
		})
	));
	assert!(matches!(
		view.lookup(&link.node, LookupName::Utf8(TARGET_NAME)),
		Err(ViewError::WrongKind {
			actual: NodeKind::Symlink,
			..
		})
	));
	assert!(matches!(
		view.read_dir(&link.node),
		Err(ViewError::WrongKind {
			actual: NodeKind::Symlink,
			..
		})
	));

	let empty = view
		.lookup(&root, LookupName::Utf8(EMPTY_NAME))
		.unwrap()
		.unwrap();
	assert_eq!(
		view.with_fork(&empty.node, ForkKind::Data, |fork| fork.is_empty())
			.unwrap(),
		Some(true)
	);
	assert_eq!(
		view.with_fork(&empty.node, ForkKind::Resource, |_| ())
			.unwrap(),
		None
	);
	assert!(matches!(
		view.read_link(&empty.node),
		Err(ViewError::WrongKind {
			actual: NodeKind::RegularFile,
			..
		})
	));
}

fn open_view<S: ReadAt>(
	container: &Container<S>,
	limits: TreeLimits,
) -> VolumeView<ApfsBackend<'_, S>>
where
	S::Error: StdError + 'static,
{
	let native = NativeView::open(container, APFS_VOLUME, LookupOptions::default(), limits)
		.unwrap()
		.unwrap();
	assert_eq!(native.volume().checkpoint, SELECTED_XID);
	VolumeView::new(VOLUME, ApfsBackend::new(native))
}

fn assert_fork<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	handle: &NodeHandle<B::Id>,
	kind: ForkKind,
	expected: &[u8],
) {
	view.with_fork(handle, kind, |reader| {
		let mut middle = [0; BOUNDARY_BYTES];
		reader.read_exact_at(BOUNDARY_OFFSET, &mut middle).unwrap();
		assert_eq!(
			middle,
			expected[BOUNDARY_OFFSET as usize..BOUNDARY_OFFSET as usize + BOUNDARY_BYTES]
		);
		assert_eq!(reader.len(), expected.len() as u64);
		let mut all = vec![0; expected.len()];
		reader.read_exact_at(0, &mut all).unwrap();
		assert_eq!(all, expected);
		reader.read_exact_at(reader.len(), &mut []).unwrap();
		assert!(matches!(
			reader.read_exact_at(reader.len(), &mut middle),
			Err(ViewError::OutOfRange { .. })
		));
	})
	.unwrap()
	.unwrap();
}
