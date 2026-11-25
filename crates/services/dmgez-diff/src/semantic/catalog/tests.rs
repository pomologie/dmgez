use std::{collections::BTreeMap, convert::Infallible, slice::Iter};

use dmgez_vfs::{
	VolumeId,
	volume::{DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, LookupName},
};

use super::{
	Catalog, CatalogError, Limits, Metadata, NodeKind, NonZeroU16, RawName, VolumeBackend,
	VolumeView, collect,
};

const ROOT: u32 = 0;
const NESTED: u32 = 1;
const FIRST_FILE: u32 = 2;
const SECOND_FILE: u32 = 3;
const THIRD_FILE: u32 = 4;

#[test]
fn matches_exact_stored_text_and_keeps_components_separate() {
	let fixture = Fixture::new([
		(
			ROOT,
			vec![
				file("z", FIRST_FILE),
				child(text("a"), NESTED, NodeKind::Directory),
			],
		),
		(
			NESTED,
			vec![
				child(
					RawName::Utf16("é/x".encode_utf16().collect()),
					SECOND_FILE,
					NodeKind::RegularFile,
				),
				file("e\u{301}/x", THIRD_FILE),
			],
		),
	]);
	let catalog = admit(fixture, Limits::default()).unwrap();
	let paths = catalog
		.keys()
		.map(|parts| parts.iter().map(String::as_str).collect::<Vec<_>>())
		.collect::<Vec<_>>();
	assert_eq!(
		paths,
		[
			vec![],
			vec!["a"],
			vec!["a", "e\u{301}/x"],
			vec!["a", "é/x"],
			vec!["z"]
		]
	);
}

#[test]
fn refuses_duplicate_text_and_invalid_utf16() {
	let duplicate = Fixture::new([(
		ROOT,
		vec![
			file("x", FIRST_FILE),
			child(
				RawName::Utf16("x".encode_utf16().collect()),
				SECOND_FILE,
				NodeKind::RegularFile,
			),
		],
	)]);
	assert!(matches!(
		admit(duplicate, Limits::default()),
		Err(CatalogError::DuplicatePath)
	));
	let lone_high_surrogate = 0xd800;
	let invalid = Fixture::new([(
		ROOT,
		vec![child(
			RawName::Utf16(vec![lone_high_surrogate]),
			FIRST_FILE,
			NodeKind::RegularFile,
		)],
	)]);
	assert!(matches!(
		admit(invalid, Limits::default()),
		Err(CatalogError::UnsupportedName)
	));
}

#[test]
fn refuses_directory_cycles_and_stalled_pages() {
	let cycle = Fixture::new([(ROOT, vec![child(text("cycle"), ROOT, NodeKind::Directory)])]);
	assert!(matches!(
		admit(cycle, Limits::default()),
		Err(CatalogError::RepeatedDirectory)
	));
	let stalled = Fixture {
		stalled: true,
		..Fixture::new([])
	};
	assert!(matches!(
		admit(stalled, Limits::default()),
		Err(CatalogError::StalledDirectory)
	));
}

#[test]
fn charges_root_entries_depth_and_full_paths_at_exact_boundaries() {
	let fixture = Fixture::new([
		(ROOT, vec![child(text("a"), NESTED, NodeKind::Directory)]),
		(NESTED, vec![file("xy", FIRST_FILE)]),
	]);
	let exact = Limits {
		max_entries: 3,
		max_depth: 2,
		// /, /a, and /a/xy consume one, two, and five bytes.
		max_path_bytes: 8,
		..Limits::default()
	};
	assert_eq!(admit(fixture.clone(), exact).unwrap().len(), 3);
	assert!(matches!(
		admit(
			fixture.clone(),
			Limits {
				max_entries: 2,
				..exact
			}
		),
		Err(CatalogError::EntryLimit { limit: 2 })
	));
	assert!(matches!(
		admit(
			fixture.clone(),
			Limits {
				max_depth: 1,
				..exact
			}
		),
		Err(CatalogError::DepthLimit { limit: 1 })
	));
	assert!(matches!(
		admit(
			fixture,
			Limits {
				max_path_bytes: 7,
				..exact
			}
		),
		Err(CatalogError::PathBytesLimit { limit: 7 })
	));
}

fn admit(fixture: Fixture, limits: Limits) -> Result<Catalog<u32>, CatalogError<Infallible>> {
	collect(
		&mut VolumeView::new(VolumeId::WholeDisk, fixture),
		limits,
		&mut || false,
	)
}

fn text(text: &str) -> RawName {
	RawName::Utf8(text.into())
}

fn file(name: &str, id: u32) -> DirectoryEntry<u32> {
	child(text(name), id, NodeKind::RegularFile)
}

fn child(name: RawName, node: u32, kind: NodeKind) -> DirectoryEntry<u32> {
	DirectoryEntry { name, node, kind }
}

#[derive(Clone)]
struct Fixture {
	directories: BTreeMap<u32, Vec<DirectoryEntry<u32>>>,
	stalled: bool,
}

impl Fixture {
	fn new(directories: impl IntoIterator<Item = (u32, Vec<DirectoryEntry<u32>>)>) -> Self {
		Self {
			directories: directories.into_iter().collect(),
			stalled: false,
		}
	}
}

struct Directory<'entry> {
	children: Iter<'entry, DirectoryEntry<u32>>,
	stalled: bool,
}

impl DirectoryReader for Directory<'_> {
	type Id = u32;
	type Error = Infallible;

	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<u32>, Infallible> {
		let entries = self
			.children
			.by_ref()
			.take(usize::from(limit.get()))
			.cloned()
			.collect();
		Ok(DirectoryPage {
			entries,
			complete: !self.stalled && self.children.as_slice().is_empty(),
		})
	}
}

impl VolumeBackend for Fixture {
	type Id = u32;
	type Error = Infallible;
	type Directory<'view> = Directory<'view>;

	fn root(&mut self) -> Result<u32, Infallible> {
		Ok(ROOT)
	}

	fn lookup(
		&mut self,
		_parent: u32,
		_name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<u32>>, Infallible> {
		Ok(None)
	}

	fn stat(&mut self, node: u32) -> Result<Option<Metadata<u64>>, Infallible> {
		let kind = if node == ROOT || self.directories.contains_key(&node) {
			NodeKind::Directory
		} else {
			NodeKind::RegularFile
		};
		Ok(Some(Metadata {
			kind,
			mode: 0,
			uid: 0,
			gid: 0,
			hard_link: None,
		}))
	}

	fn read_dir(&mut self, directory: u32) -> Result<Self::Directory<'_>, Infallible> {
		let children = self
			.directories
			.get(&directory)
			.map_or(&[][..], Vec::as_slice)
			.iter();
		Ok(Directory {
			children,
			stalled: self.stalled,
		})
	}

	fn with_fork<T>(
		&mut self,
		_node: u32,
		_kind: ForkKind,
		_read: impl FnOnce(&mut dyn ForkReader<Error = Infallible>) -> T,
	) -> Result<Option<T>, Infallible> {
		Ok(None)
	}

	fn read_link(&mut self, _node: u32) -> Result<Option<Vec<u8>>, Infallible> {
		Ok(None)
	}
}
