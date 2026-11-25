use std::num::NonZeroU16;

use dmgez_hfsplus_read_probe::{Error, HfsView, Limits, with_view};
use dmgez_operations::extract::{NameLimits, NameProfile, preflight_names};
use dmgez_udif::{reader, writer};
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{ForkKind, LookupName, NodeKind, RawName, ViewError},
};
use hfsplus_format::{
	hard_links::PRIVATE_DIRECTORY_NAME,
	volume::{CLEAN_UNMOUNT_ATTRIBUTE, Geometry, PRIMARY_HEADER_OFFSET},
};
use hfsplus_reader::{ForkReadSupport, catalog::CatalogError};

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "native/extract_host.rs"]
mod extract_host;

const HARD_LINKS: &[u8] = include_bytes!("fixtures/native-chained-hardlinks.bin");
const CLEAN_JOURNAL: &[u8] = include_bytes!("fixtures/native-journal.bin");
const DATA: &[u8] = b"pomologie linked file\n";
const RESOURCE: &[u8] = b"pomologie resource\0fork\n";
const JOURNALED_DATA: &[u8] = b"journal admission fixture\n";
const ROOT_NAMES: [&str; 4] = ["four", "nested", "one", "two"];
const NATIVE_MODE: u16 = 0o100751;
const NATIVE_UID: u32 = 501;
const NATIVE_GID: u32 = 20;
const NATIVE_LINK_COUNT: u32 = 3;
const HFS_BLOCK_BYTES: u32 = 4096;
const HFS_BLOCK_COUNT: u32 = 2048;
const HFS_HEADER_ATTRIBUTES: usize = PRIMARY_HEADER_OFFSET as usize + size_of::<u32>();

fn materialize(mut fixture: &[u8]) -> Vec<u8> {
	fn next<const BYTES: usize>(source: &mut &[u8]) -> [u8; BYTES] {
		let (field, remaining) = source.split_first_chunk().unwrap();
		*source = remaining;
		*field
	}
	let length = usize::try_from(u64::from_be_bytes(next(&mut fixture))).unwrap();
	let count = u16::from_be_bytes(next(&mut fixture));
	let mut disk = vec![0; length];
	for _ in 0..count {
		let start = usize::try_from(u64::from_be_bytes(next(&mut fixture))).unwrap();
		let width = usize::try_from(u32::from_be_bytes(next(&mut fixture))).unwrap();
		let (bytes, remaining) = fixture.split_at(width);
		disk[start..start + width].copy_from_slice(bytes);
		fixture = remaining;
	}
	assert!(fixture.is_empty());
	disk
}

fn udzo(logical: &[u8]) -> Vec<u8> {
	let mut image = Vec::new();
	writer::write_image(
		logical,
		logical.len() as u64,
		&mut image,
		&writer::WriteOptions::default(),
	)
	.unwrap();
	image
}

fn disk(image: &[u8]) -> Disk<reader::Image<&[u8]>> {
	let source = reader::Image::open(image, reader::Limits::default()).unwrap();
	Disk::discover(source, dmgez_vfs::Limits::default()).unwrap()
}

fn with_image(logical: &[u8], check: impl FnOnce(&mut HfsView<'_, '_, reader::Image<&[u8]>>)) {
	let image = udzo(logical);
	let mut disk = disk(&image);
	let selected = disk.select_volume(None).unwrap();
	assert_eq!(selected, VolumeId::WholeDisk);
	assert_eq!(
		disk.data_window(selected).unwrap().len(),
		logical.len() as u64
	);
	with_view(&mut disk, selected, Limits::default(), check).unwrap();
}

fn names(view: &mut HfsView<'_, '_, reader::Image<&[u8]>>) -> Vec<RawName> {
	let root = view.root().unwrap();
	let mut entries = Vec::new();
	let mut scan = view.read_dir(&root).unwrap();
	loop {
		let page_size = NonZeroU16::MIN;
		let page = scan.next_page(page_size).unwrap();
		assert!(page.entries.len() <= usize::from(page_size.get()));
		entries.extend(page.entries.into_iter().map(|entry| entry.name));
		if page.complete {
			return entries;
		}
	}
}

#[test]
fn named_hard_links_keep_separate_catalog_ids_and_share_both_forks() {
	with_image(&materialize(HARD_LINKS), |view| {
		let root = view.root().unwrap();
		assert_eq!(root, view.root().unwrap());
		assert_eq!(view.stat(&root).unwrap().kind, NodeKind::Directory);
		let entries = names(view);
		let expected: Vec<_> = ROOT_NAMES
			.into_iter()
			.map(|name| RawName::Utf16(name.encode_utf16().collect()))
			.collect();
		assert_eq!(entries, expected);
		assert!(
			view.lookup(&root, LookupName::StoredUtf16(&PRIVATE_DIRECTORY_NAME))
				.unwrap()
				.is_none()
		);

		let one = view
			.lookup(&root, entries[2].as_lookup())
			.unwrap()
			.unwrap()
			.node;
		let two = view
			.lookup(&root, LookupName::Utf8("two"))
			.unwrap()
			.unwrap()
			.node;
		let nested = view
			.lookup(&root, LookupName::Utf8("nested"))
			.unwrap()
			.unwrap()
			.node;
		let three = view
			.lookup(&nested, LookupName::Utf8("three"))
			.unwrap()
			.unwrap()
			.node;
		assert_ne!(one, two);
		assert_ne!(two, three);
		assert_ne!(one, three);
		let metadata = view.stat(&one).unwrap();
		assert_eq!(
			(metadata.mode, metadata.uid, metadata.gid),
			(NATIVE_MODE, NATIVE_UID, NATIVE_GID)
		);
		assert_eq!(
			metadata.hard_link.as_ref().unwrap().count.get(),
			NATIVE_LINK_COUNT
		);
		assert_eq!(metadata, view.stat(&two).unwrap());
		assert_eq!(metadata, view.stat(&three).unwrap());
		for entry in [&one, &two, &three] {
			for (kind, expected) in [(ForkKind::Data, DATA), (ForkKind::Resource, RESOURCE)] {
				view.with_fork(entry, kind, |fork| {
					assert_eq!(fork.len(), expected.len() as u64);
					let mut actual = vec![0; expected.len()];
					fork.read_exact_at(0, &mut actual).unwrap();
					assert_eq!(actual, expected);
					fork.read_exact_at(fork.len(), &mut []).unwrap();
					assert!(matches!(
						fork.read_exact_at(fork.len(), &mut [0]),
						Err(ViewError::OutOfRange { .. })
					));
				})
				.unwrap()
				.unwrap();
			}
		}
		let four = view
			.lookup(&root, entries[0].as_lookup())
			.unwrap()
			.unwrap()
			.node;
		assert_ne!(metadata.hard_link, view.stat(&four).unwrap().hard_link);
		assert_eq!(
			view.with_fork(&four, ForkKind::Resource, |fork| fork.is_empty())
				.unwrap(),
			Some(true)
		);
	});
}

#[test]
fn clean_native_journal_preserves_visible_catalog_and_exact_data() {
	const MESSAGE: &str = "message.txt";
	with_image(&materialize(CLEAN_JOURNAL), |view| {
		let root = view.root().unwrap();
		assert_eq!(
			names(view),
			[RawName::Utf16(MESSAGE.encode_utf16().collect())]
		);
		for private in [".journal", ".journal_info_block"] {
			assert!(
				view.lookup(&root, LookupName::Utf8(private))
					.unwrap()
					.is_none()
			);
		}
		let message = view
			.lookup(&root, LookupName::Utf8(MESSAGE))
			.unwrap()
			.unwrap()
			.node;
		view.with_fork(&message, ForkKind::Data, |fork| {
			let mut actual = [0; JOURNALED_DATA.len()];
			assert_eq!(fork.len(), actual.len() as u64);
			fork.read_exact_at(0, &mut actual).unwrap();
			assert_eq!(actual, JOURNALED_DATA);
		})
		.unwrap()
		.unwrap();
	});
}

#[test]
fn refusing_a_dirty_journal_prevents_view_access() {
	let mut logical = materialize(CLEAN_JOURNAL);
	let stored = &mut logical[HFS_HEADER_ATTRIBUTES..][..size_of::<u32>()];
	let generic_flags = u32::from_be_bytes(stored.try_into().unwrap());
	stored.copy_from_slice(&(generic_flags & !CLEAN_UNMOUNT_ATTRIBUTE).to_be_bytes());
	let image = udzo(&logical);
	let mut disk = disk(&image);
	let selected = disk.select_volume(None).unwrap();
	assert!(matches!(
		with_view(&mut disk, selected, Limits::default(), |_| panic!(
			"dirty volume opened"
		)),
		Err(Error::Catalog(CatalogError::Read(
			hfsplus_reader::Error::UnsupportedVolume(ForkReadSupport::NeedsJournalReplay,)
		)))
	));
}

#[cfg(feature = "hfsx-fixture")]
#[test]
fn native_hfsx_root_case_collision_preserves_posix_spellings() {
	use dmgez_operations::extract::NamePlanError;
	const NATIVE_HFSX: &[u8] = include_bytes!("fixtures/native-hfsx.bin");
	const DIRECTORIES: [&str; 2] = ["Cases", "cases"];
	const CASE_PAIR: [&str; 2] = ["Case.txt", "case.txt"];
	let logical = materialize(NATIVE_HFSX);
	with_image(&logical, |view| {
		let root = view.root().unwrap();
		let limits = NameLimits {
			page_entries: NonZeroU16::MIN,
			..NameLimits::default()
		};
		let Err(NamePlanError::Collision { first, second }) =
			preflight_names(view, &root, NameProfile::PortableAscii, limits, || false)
		else {
			panic!("native directory names must collide in the portable profile");
		};
		assert_eq!([first.as_str(), second.as_str()], DIRECTORIES);

		let plan = preflight_names(view, &root, NameProfile::PosixUtf8, limits, || false).unwrap();
		let roots: Vec<_> = plan
			.entries()
			.iter()
			.filter(|entry| entry.parent.is_none())
			.map(|entry| entry.name.as_str())
			.collect();
		assert_eq!(roots, DIRECTORIES);
		let files = CASE_PAIR.map(|name| {
			let entry = plan
				.entries()
				.iter()
				.find(|entry| entry.name.as_str() == name)
				.unwrap();
			let parent = &plan.entries()[entry.parent.unwrap().index()];
			assert_eq!(parent.name.as_str(), DIRECTORIES[0]);
			assert_eq!(entry.kind, NodeKind::RegularFile);
			&entry.source
		});
		assert_ne!(files[0], files[1]);
	});
}

#[test]
fn constructed_symlink_preserves_non_utf8_target_without_dereferencing() {
	use hfsplus_builder::volume::{
		EntryKind, EntryMetadata, ManifestEntry, VolumeOptions, plan_ascii,
	};
	const LINK_NAME: &str = "Relative";
	const TARGET: &[u8] = b"../missing/\xff";
	let geometry = Geometry::new(
		HFS_BLOCK_BYTES,
		HFS_BLOCK_COUNT,
		u64::from(HFS_BLOCK_BYTES) * u64::from(HFS_BLOCK_COUNT),
	)
	.unwrap();
	let link = ManifestEntry::<&[u8]> {
		path: LINK_NAME,
		metadata: EntryMetadata::new(0o751),
		kind: EntryKind::Symlink {
			target: TARGET.to_vec(),
		},
	};
	let plan = plan_ascii(VolumeOptions::new("VFS link", geometry), vec![link])
		.unwrap()
		.seal()
		.unwrap();
	let mut logical = Vec::new();
	plan.write_to(&mut logical).unwrap();
	with_image(&logical, |view| {
		let root = view.root().unwrap();
		let link = view
			.lookup(&root, LookupName::Utf8(LINK_NAME))
			.unwrap()
			.unwrap()
			.node;
		assert_eq!(view.stat(&link).unwrap().kind, NodeKind::Symlink);
		let plan = preflight_names(
			view,
			&root,
			NameProfile::PortableAscii,
			NameLimits::default(),
			|| false,
		)
		.unwrap();
		let [planned] = plan.entries() else {
			panic!("expected one planned symlink")
		};
		assert_eq!(planned.name.as_str(), LINK_NAME);
		assert_eq!(planned.kind, NodeKind::Symlink);
		assert_eq!(planned.source, link);
		assert!(planned.parent.is_none());
		assert_eq!(view.read_link(&link).unwrap(), TARGET);
		assert!(matches!(
			view.with_fork(&link, ForkKind::Data, |_| ()),
			Err(ViewError::WrongKind { .. })
		));
		assert!(matches!(
			view.lookup(&link, LookupName::Utf8("child")),
			Err(ViewError::WrongKind { .. })
		));
	});
}
