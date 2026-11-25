//! Name and topology preflight cases from D-READ-05-T01.

#![cfg(feature = "extract")]

use std::{convert::Infallible, num::NonZeroU16};

use dmgez_operations::extract::{
	NameBudget, NameError, NameLimits, NamePlan, NamePlanError, NameProfile, preflight_names,
};
use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, LookupName, Metadata,
		NodeKind, RawName, ViewError, VolumeBackend, VolumeView,
	},
};

const ROOT: usize = 0;
const FIRST_FOLDER: usize = 1;
const SECOND_FOLDER: usize = 2;
const FILE: usize = 3;
const LINK: usize = 4;
const ONE_ENTRY: NonZeroU16 = NonZeroU16::new(1).unwrap();
const UNPAIRED_SURROGATE: u16 = 0xd800;

struct Tree {
	directories: Vec<Vec<DirectoryEntry<usize>>>,
	stalled: bool,
}

struct Directory<'tree> {
	children: &'tree [DirectoryEntry<usize>],
	offset: usize,
	stalled: bool,
}

impl DirectoryReader for Directory<'_> {
	type Id = usize;
	type Error = Infallible;

	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<usize>, Infallible> {
		let end = (self.offset + usize::from(limit.get())).min(self.children.len());
		let entries = self.children[self.offset..end].to_vec();
		self.offset = end;
		Ok(DirectoryPage {
			entries,
			complete: !self.stalled && end == self.children.len(),
		})
	}
}

impl VolumeBackend for Tree {
	type Id = usize;
	type Error = Infallible;
	type Directory<'view> = Directory<'view>;

	fn root(&mut self) -> Result<usize, Infallible> {
		Ok(ROOT)
	}

	fn lookup(
		&mut self,
		_parent: usize,
		_name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<usize>>, Infallible> {
		panic!("name preflight must not resolve alternate names")
	}

	fn stat(&mut self, _node: usize) -> Result<Option<Metadata<u64>>, Infallible> {
		panic!("name preflight does not admit file metadata")
	}

	fn read_dir(&mut self, directory: usize) -> Result<Directory<'_>, Infallible> {
		Ok(Directory {
			children: &self.directories[directory],
			offset: 0,
			stalled: self.stalled,
		})
	}

	fn with_fork<T>(
		&mut self,
		_node: usize,
		_kind: ForkKind,
		_read: impl FnOnce(&mut dyn ForkReader<Error = Infallible>) -> T,
	) -> Result<Option<T>, Infallible> {
		panic!("name preflight must not open forks")
	}

	fn read_link(&mut self, _node: usize) -> Result<Option<Vec<u8>>, Infallible> {
		panic!("name preflight must not access links or their targets")
	}
}

fn edge(name: &str, node: usize, kind: NodeKind) -> DirectoryEntry<usize> {
	DirectoryEntry {
		name: RawName::Utf8(name.into()),
		node,
		kind,
	}
}

fn view(directories: Vec<Vec<DirectoryEntry<usize>>>) -> VolumeView<Tree> {
	VolumeView::new(
		VolumeId::WholeDisk,
		Tree {
			directories,
			stalled: false,
		},
	)
}

fn plan(
	directories: Vec<Vec<DirectoryEntry<usize>>>,
	profile: NameProfile,
	limits: NameLimits,
) -> Result<NamePlan<usize>, NamePlanError<Infallible>> {
	let mut view = view(directories);
	let root = view.root().unwrap();
	preflight_names(&mut view, &root, profile, limits, || false)
}

#[test]
fn confines_components_and_applies_the_portable_filename_rules() {
	for (name, expected) in [
		("", NameError::NotComponent),
		(".", NameError::NotComponent),
		("..", NameError::NotComponent),
		("/absolute", NameError::NotComponent),
		("parent/child", NameError::NotComponent),
		("missing\0suffix", NameError::NotComponent),
		("C:\\test", NameError::ForbiddenCharacter),
		("name:fork", NameError::ForbiddenCharacter),
		("one\\two", NameError::ForbiddenCharacter),
		("line\nfeed", NameError::ForbiddenCharacter),
		("trailing.", NameError::TrimmedName),
		("trailing ", NameError::TrimmedName),
		(" leading", NameError::TrimmedName),
		("nUl.txt.bak", NameError::DeviceName),
		("COM0.ext", NameError::DeviceName),
		("COM9.ext", NameError::DeviceName),
		("lpt1 .txt", NameError::DeviceName),
		("conout$", NameError::DeviceName),
		("COM¹", NameError::Encoding),
	] {
		let result = plan(
			vec![vec![edge(name, FILE, NodeKind::RegularFile)]],
			NameProfile::PortableAscii,
			NameLimits::default(),
		);
		assert!(
			matches!(result, Err(NamePlanError::Name { reason, .. }) if reason == expected),
			"{name:?}: {result:?}"
		);
	}
}

#[test]
fn uses_the_selected_case_policy_and_keeps_collisions_directory_local() {
	let entries = vec![
		edge("Readme", FILE, NodeKind::RegularFile),
		edge("README", SECOND_FOLDER, NodeKind::Directory),
	];
	let tree = vec![entries, vec![], vec![]];
	let limits = NameLimits {
		page_entries: ONE_ENTRY,
		..NameLimits::default()
	};
	let result = plan(tree.clone(), NameProfile::PortableAscii, limits);
	assert!(
		matches!(result, Err(NamePlanError::Collision { first, second }) if first.as_str() == "Readme" && second.as_str() == "README")
	);
	assert_eq!(
		plan(tree, NameProfile::PosixUtf8, limits)
			.unwrap()
			.entries()
			.len(),
		2
	);

	let tree = vec![
		vec![
			edge("first", FIRST_FOLDER, NodeKind::Directory),
			edge("second", SECOND_FOLDER, NodeKind::Directory),
		],
		vec![edge("Name", FILE, NodeKind::RegularFile)],
		vec![edge("name", LINK, NodeKind::Symlink)],
	];
	let plan = plan(tree, NameProfile::PortableAscii, limits).unwrap();
	assert_eq!(plan.profile(), NameProfile::PortableAscii);
	let entries = plan.entries();
	assert_eq!(entries.len(), 4);
	for entry in &entries[2..] {
		let parent = &entries[entry.parent.unwrap().index()];
		assert_eq!(parent.kind, NodeKind::Directory);
		assert_eq!(
			parent.name.as_str(),
			if entry.kind == NodeKind::Symlink {
				"second"
			} else {
				"first"
			}
		);
	}
}

#[test]
fn preserves_exact_utf8_spelling_and_rejects_invalid_native_units() {
	const COMPOSED: &str = "Café";
	const DECOMPOSED: &str = "Cafe\u{301}";
	let original = DirectoryEntry {
		name: RawName::Utf16(DECOMPOSED.encode_utf16().collect()),
		node: FILE,
		kind: NodeKind::RegularFile,
	};
	let result = plan(
		vec![vec![original, edge(COMPOSED, LINK, NodeKind::RegularFile)]],
		NameProfile::PosixUtf8,
		NameLimits::default(),
	)
	.unwrap();
	assert_eq!(result.entries()[0].name.as_str(), DECOMPOSED);
	assert_eq!(result.entries()[1].name.as_str(), COMPOSED);
	let invalid = DirectoryEntry {
		name: RawName::Utf16(vec![UNPAIRED_SURROGATE]),
		node: FILE,
		kind: NodeKind::RegularFile,
	};
	assert!(matches!(
		plan(
			vec![vec![invalid]],
			NameProfile::PosixUtf8,
			NameLimits::default()
		),
		Err(NamePlanError::Name {
			reason: NameError::Encoding,
			..
		})
	));
}

#[test]
fn bounds_repeated_directories_pages_depth_and_names() {
	let tree = vec![vec![edge("cycle", ROOT, NodeKind::Directory)]];
	let limits = NameLimits::default();
	assert!(matches!(
		plan(tree, NameProfile::PortableAscii, limits),
		Err(NamePlanError::RepeatedDirectory)
	));
	let tree = vec![
		vec![edge("dir", FIRST_FOLDER, NodeKind::Directory)],
		vec![edge("a", FILE, NodeKind::RegularFile)],
	];
	for (limits, expected) in [
		(
			NameLimits {
				max_entries: 1,
				..limits
			},
			NameBudget::Entries,
		),
		(
			NameLimits {
				max_depth: 1,
				..limits
			},
			NameBudget::Depth,
		),
		(
			NameLimits {
				max_relative_path_bytes: 4,
				..limits
			},
			NameBudget::RelativePathBytes,
		),
		(
			NameLimits {
				max_total_name_bytes: 3,
				..limits
			},
			NameBudget::TotalNameBytes,
		),
	] {
		assert!(
			matches!(plan(tree.clone(), NameProfile::PortableAscii, limits), Err(NamePlanError::Limit(actual)) if actual == expected)
		);
	}
	assert!(matches!(
		plan(
			tree,
			NameProfile::PortableAscii,
			NameLimits {
				max_component_bytes: 2,
				..limits
			}
		),
		Err(NamePlanError::Name {
			reason: NameError::TooLong,
			..
		})
	));
	let mut stalled = VolumeView::new(
		VolumeId::WholeDisk,
		Tree {
			directories: vec![vec![]],
			stalled: true,
		},
	);
	let root = stalled.root().unwrap();
	assert!(matches!(
		preflight_names(
			&mut stalled,
			&root,
			NameProfile::PortableAscii,
			NameLimits {
				max_pages: 3,
				..limits
			},
			|| false
		),
		Err(NamePlanError::Limit(NameBudget::Pages))
	));
}

#[test]
fn rejects_foreign_directories_and_stops_on_cancellation() {
	let foreign = view(vec![vec![]]).root().unwrap();
	let mut own = view(vec![vec![edge("file", FILE, NodeKind::RegularFile)]]);
	let limits = NameLimits::default();
	assert!(matches!(
		preflight_names(
			&mut own,
			&foreign,
			NameProfile::PortableAscii,
			limits,
			|| false
		),
		Err(NamePlanError::Source(ViewError::ForeignHandle))
	));
	let mut remaining_polls = 2;
	let root = own.root().unwrap();
	assert!(matches!(
		preflight_names(&mut own, &root, NameProfile::PortableAscii, limits, || {
			remaining_polls -= 1;
			remaining_polls == 0
		}),
		Err(NamePlanError::Cancelled)
	));
}
