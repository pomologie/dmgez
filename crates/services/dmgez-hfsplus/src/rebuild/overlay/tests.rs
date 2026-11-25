use super::super::{File, Payload, capture::MAX_DEPTH};
use super::*;
use dmgez_config::project_edit::{ContentChanges, FieldChange};
use dmgez_finder::{
	edit::{EditableStore, IconLocationRecord},
	inspection::ReadLimits,
	write_store,
};
use hfsplus_core::ByteOffset;
use std::rc::Rc;

type TestSnapshot = Snapshot<'static, &'static [u8]>;
type TestFile = File<'static, &'static [u8]>;

fn path(value: &str) -> VirtualPath {
	value.parse().unwrap()
}

fn snapshot() -> TestSnapshot {
	let mut entries = BTreeMap::new();
	for value in ["/Left", "/Right", "/Discard", "/Discard/Nested"] {
		entries.insert(
			path(value),
			Node {
				metadata: EntryMetadata::new(DIRECTORY_MODE),
				kind: Kind::Directory,
				attributes: Default::default(),
			},
		);
	}
	for (value, target) in [
		("/Left/E\u{301}", b"hard\xfftarget".as_slice()),
		("/Right/Shortcut", b"other".as_slice()),
		("/Discard/Nested/Shortcut", b"pruned".as_slice()),
	] {
		entries.insert(
			path(value),
			Node {
				metadata: EntryMetadata::new(SYMLINK_MODE),
				kind: Kind::Symlink(target.to_vec()),
				attributes: Default::default(),
			},
		);
	}
	TestSnapshot {
		name: "Original".into(),
		root: EntryMetadata::new(DIRECTORY_MODE),
		dates: [0; 4],
		finder_info: [0; 32],
		block_size: 4096,
		entries,
		root_attributes: Vec::new(),
		backgrounds: BTreeMap::new(),
	}
}

fn rename(target: &str, destination: &str) -> ProjectEdit {
	let mut changes = ContentChanges::default();
	changes.destination = Some(path(destination));
	ProjectEdit::Modify {
		target: path(target),
		changes,
	}
}

fn remove(target: &str) -> ProjectEdit {
	ProjectEdit::Remove {
		target: path(target),
	}
}

fn add_directory(destination: &str) -> ProjectEdit {
	ProjectEdit::Add(ContentDraft::Directory {
		destination: path(destination),
	})
}

fn copy(destination: &str) -> ProjectEdit {
	ProjectEdit::Add(ContentDraft::Copy {
		source: "host".into(),
		destination: Some(path(destination)),
	})
}

fn captured_file(destination: &str, bytes: &[u8]) -> (VirtualPath, Node<'static, &'static [u8]>) {
	(
		path(destination),
		Node {
			metadata: EntryMetadata::new(0o640),
			kind: Kind::File {
				file: Rc::new(File {
					data: Payload::bytes(bytes.to_vec()),
					resource: None,
				}),
				inode: None,
			},
			attributes: Default::default(),
		},
	)
}

fn position(target: &str, position: FieldChange<[f64; 2]>) -> ProjectEdit {
	let mut changes = ContentChanges::default();
	changes.position = position;
	ProjectEdit::Modify {
		target: path(target),
		changes,
	}
}

fn add_store(image: &mut TestSnapshot, name: &str, owner: u32, positions: &[(&str, u32, u32)]) {
	let default = write_store(None, &[]).unwrap();
	let layout = EditableStore::open(&default, ReadLimits::default()).unwrap();
	let positions = positions
		.iter()
		.map(|&(name, x, y)| (name.to_owned(), IconLocationRecord::new(x, y)))
		.collect();
	let bytes = layout.encode(&positions).unwrap().unwrap_or(default);
	let layout = EditableStore::open(&bytes, ReadLimits::default()).unwrap();
	let mut metadata = EntryMetadata::new(0o644);
	metadata.uid = owner;
	image.entries.insert(
		path(name),
		Node {
			metadata,
			kind: Kind::FinderStore {
				file: Rc::new(File {
					data: Payload::bytes(bytes),
					resource: None,
				}),
				layout,
			},
			attributes: Default::default(),
		},
	);
}

fn store<'a>(image: &'a TestSnapshot, name: &str) -> (&'a Rc<TestFile>, &'a EditableStore) {
	match &image.entries[&path(name)].kind {
		Kind::FinderStore { file, layout } => (file, layout),
		_ => panic!("expected an admitted Finder store at {name}"),
	}
}

fn store_bytes(image: &TestSnapshot, name: &str) -> Vec<u8> {
	let (file, _) = store(image, name);
	let mut bytes = vec![0; usize::try_from(file.data.len().0).unwrap()];
	file.data.read_exact_at(ByteOffset(0), &mut bytes).unwrap();
	bytes
}

fn locations<'a>(image: &'a TestSnapshot, name: &str) -> Vec<(&'a str, u32, u32)> {
	store(image, name)
		.1
		.positions()
		.map(|(name, record)| (name, record.x, record.y))
		.collect()
}

fn target<'a>(snapshot: &'a TestSnapshot, value: &str) -> &'a [u8] {
	let Kind::Symlink(target) = &snapshot.entries[&path(value)].kind else {
		panic!("expected a symlink at {value}");
	};
	target
}

#[test]
fn addresses_original_catalog_entries_during_a_directory_swap() {
	let mut image = snapshot();
	apply(
		&mut image,
		&[
			rename("/left", "/Right"),
			rename("/Right", "/Left"),
			rename("/LEFT/é", "/Right/Renamed"),
			remove("/Discard"),
			ProjectEdit::SetImageName("After".into()),
		],
		Path::new("."),
	)
	.unwrap();
	assert_eq!(image.name, "After");
	assert_eq!(target(&image, "/Right/Renamed"), b"hard\xfftarget");
	assert_eq!(target(&image, "/Left/Shortcut"), b"other");
	assert_eq!(image.entries.len(), 4);
}

#[test]
fn permits_replacement_after_removal_and_requires_directory_parents() {
	let mut image = snapshot();
	let edits = [
		ProjectEdit::Add(ContentDraft::Symlink {
			destination: path("/Discard/Applications"),
			target: "/Applications".into(),
		}),
		remove("/Discard"),
		add_directory("/Discard"),
	];
	apply(&mut image, &edits, Path::new(".")).unwrap();
	assert_eq!(target(&image, "/Discard/Applications"), b"/Applications");
	assert!(!image.entries.contains_key(&path("/Discard/Nested")));
	assert!(matches!(
		apply(
			&mut image,
			&[add_directory("/discard/applications/NotADirectory"),],
			Path::new(".")
		),
		Err(OverlayError::Parent { .. })
	));
}

#[test]
fn host_replacement_does_not_inherit_the_removed_items_finder_position() {
	let mut image = snapshot();
	let replaced = "/Right/Shortcut";
	let payload = b"captured file bytes";
	add_store(&mut image, "/Right/.DS_Store", 31, &[("Shortcut", 11, 17)]);
	apply_with_copies(
		&mut image,
		&[copy(replaced), remove(replaced)],
		vec![captured_file(replaced, payload)],
	)
	.unwrap();
	let replacement = &image.entries[&path(replaced)];
	let Kind::File { file, .. } = &replacement.kind else {
		panic!("expected the captured replacement file");
	};
	let mut actual = [0; b"captured file bytes".len()];
	file.data.read_exact_at(ByteOffset(0), &mut actual).unwrap();
	assert_eq!(&actual, payload);
	assert_eq!(replacement.metadata.mode, 0o640);
	assert!(locations(&image, "/Right/.DS_Store").is_empty());
}

#[test]
fn admitted_host_nodes_share_native_collision_and_parent_validation() {
	let mut image = snapshot();
	let edits = [
		ProjectEdit::SetImageName("After".into()),
		copy("/right/shortcut"),
	];
	assert!(matches!(
		apply_with_copies(
			&mut image,
			&edits,
			vec![captured_file("/right/shortcut", b"")]
		),
		Err(OverlayError::Collision { .. })
	));
	assert_eq!(image.name, "Original");
	assert_eq!(target(&image, "/Right/Shortcut"), b"other");
	let descendant = "/right/shortcut/Child";
	assert!(matches!(
		apply_with_copies(
			&mut image,
			&[copy(descendant)],
			vec![captured_file(descendant, b"")]
		),
		Err(OverlayError::Parent { .. })
	));
	let reserved = "/Right/.ds_store";
	assert!(matches!(
		apply_with_copies(
			&mut image,
			&[copy(reserved)],
			vec![captured_file(reserved, b"")]
		),
		Err(OverlayError::FinderStore { .. })
	));
	assert_eq!(image.entries.len(), 7);
}

#[test]
fn captured_nodes_require_a_matching_copy_batch() {
	let mut image = snapshot();
	assert!(matches!(
		apply_with_copies(&mut image, &[], vec![captured_file("/Admitted", b"")]),
		Err(OverlayError::UnexpectedHostCopies)
	));
	assert!(matches!(
		apply_with_copies(
			&mut image,
			&[copy("/Admitted"), copy("/Missing")],
			vec![captured_file("/Admitted", b"")]
		),
		Err(OverlayError::HostCopy { index: 1 })
	));
	assert_eq!(image.entries.len(), 7);
}

#[test]
fn collisions_and_removal_conflicts_leave_the_snapshot_untouched() {
	let mut image = snapshot();
	assert!(matches!(
		apply(
			&mut image,
			&[
				ProjectEdit::SetImageName("After".into()),
				rename("/Left/E\u{301}", "/RIGHT/shortcut"),
			],
			Path::new(".")
		),
		Err(OverlayError::Collision { .. })
	));
	assert_eq!(image.name, "Original");
	assert_eq!(target(&image, "/Left/E\u{301}"), b"hard\xfftarget");
	assert_eq!(image.entries.len(), 7);
	assert!(matches!(
		apply(
			&mut image,
			&[
				rename("/Discard/Nested/Shortcut", "/Escape"),
				remove("/Discard"),
			],
			Path::new(".")
		),
		Err(OverlayError::ConflictingEdit { index: 0 })
	));
	assert_eq!(target(&image, "/Discard/Nested/Shortcut"), b"pruned");
}

#[test]
fn refuses_reserved_store_paths_and_unprepared_host_copies() {
	let mut image = snapshot();
	assert!(matches!(
		apply(
			&mut image,
			&[add_directory("/Right/.ds_store"),],
			Path::new(".")
		),
		Err(OverlayError::FinderStore { .. })
	));
	assert!(matches!(
		apply(
			&mut image,
			&[ProjectEdit::Add(ContentDraft::Copy {
				source: "host".into(),
				destination: Some(path("/Copy"))
			}),],
			Path::new(".")
		),
		Err(OverlayError::HostCopy { index: 0 })
	));
	assert_eq!(image.entries.len(), 7);
}

#[test]
fn a_flag_edit_cannot_target_content_removed_with_its_parent() {
	let mut image = snapshot();
	let target = path("/Discard/Nested/Shortcut");
	let before = image.entries[&target].metadata.finder_info;
	let mut changes = ContentChanges::default();
	changes.hidden = FieldChange::Set(true);
	assert!(matches!(
		apply(
			&mut image,
			&[
				ProjectEdit::Modify {
					target: target.clone(),
					changes,
				},
				remove("/Discard"),
			],
			Path::new(".")
		),
		Err(OverlayError::ConflictingEdit { index: 0 })
	));
	assert_eq!(image.entries[&target].metadata.finder_info, before);
}

#[test]
fn a_directory_move_cannot_push_descendants_past_the_depth_budget() {
	let mut image = snapshot();
	let destination = "/parent".repeat(MAX_DEPTH);
	assert!(matches!(
		apply(&mut image, &[rename("/Left", &destination)], Path::new(".")),
		Err(OverlayError::Budget)
	));
	assert_eq!(target(&image, "/Left/E\u{301}"), b"hard\xfftarget");
}

#[test]
fn retains_the_exact_default_store_when_catalog_names_change() {
	let mut image = snapshot();
	add_store(&mut image, "/.DS_Store", 41, &[]);
	let before = Rc::clone(store(&image, "/.DS_Store").0);
	let bytes = store_bytes(&image, "/.DS_Store");
	apply(&mut image, &[rename("/Left", "/Renamed")], Path::new(".")).unwrap();
	assert!(Rc::ptr_eq(&before, store(&image, "/.DS_Store").0));
	assert_eq!(store_bytes(&image, "/.DS_Store"), bytes);
	assert_eq!(image.entries[&path("/.DS_Store")].metadata.uid, 41);
}

#[test]
fn retains_folder_layout_identity_while_remapping_icons_across_swapped_folders() {
	let mut image = snapshot();
	apply(
		&mut image,
		&[
			add_directory("/Left/Book"),
			add_directory("/Right/DiscardMe"),
		],
		Path::new("."),
	)
	.unwrap();
	add_store(&mut image, "/Left/.ds_store", 41, &[("Book", 9, 23)]);
	add_store(
		&mut image,
		"/Right/.DS_Store",
		42,
		&[("DiscardMe", 5, 7), ("shortcut", 11, 13)],
	);
	apply(
		&mut image,
		&[
			rename("/Left", "/Right"),
			rename("/Right", "/Left"),
			rename("/left/book", "/LEFT/Binder"),
			remove("/right/DiscardMe"),
			position("/right/SHORTCUT", FieldChange::Set([17.0, 19.0])),
		],
		Path::new("."),
	)
	.unwrap();
	assert_eq!(image.entries[&path("/Right/.ds_store")].metadata.uid, 41);
	assert_eq!(image.entries[&path("/Left/.DS_Store")].metadata.uid, 42);
	assert!(locations(&image, "/Right/.ds_store").is_empty());
	assert_eq!(
		locations(&image, "/Left/.DS_Store"),
		[("Binder", 9, 23), ("shortcut", 17, 19)]
	);
	assert!(!image.entries.contains_key(&path("/Left/DiscardMe")));
}

#[test]
fn creates_a_destination_store_and_clears_an_existing_location() {
	let mut image = snapshot();
	add_store(&mut image, "/Right/.DS_Store", 31, &[("Shortcut", 1, 2)]);
	apply(
		&mut image,
		&[rename("/Right/Shortcut", "/Left/Relocated")],
		Path::new("."),
	)
	.unwrap();
	assert!(locations(&image, "/Right/.DS_Store").is_empty());
	assert_eq!(locations(&image, "/Left/.DS_Store"), [("Relocated", 1, 2)]);
	apply(
		&mut image,
		&[position("/Left/Relocated", FieldChange::Clear)],
		Path::new("."),
	)
	.unwrap();
	assert!(locations(&image, "/Left/.DS_Store").is_empty());
	apply(
		&mut image,
		&[position("/Left/Relocated", FieldChange::Set([3.0, 4.0]))],
		Path::new("."),
	)
	.unwrap();
	assert_eq!(locations(&image, "/Left/.DS_Store"), [("Relocated", 3, 4)]);
}

#[test]
fn rejects_unresolved_locations_invalid_coordinates_and_explicit_store_edits() {
	let mut image = snapshot();
	add_store(&mut image, "/Left/.DS_Store", 41, &[("Missing", 9, 23)]);
	let bytes = store_bytes(&image, "/Left/.DS_Store");
	assert!(matches!(
		apply(&mut image, &[], Path::new(".")),
		Err(OverlayError::FinderRecordTarget { .. })
	));
	assert!(matches!(
		apply(&mut image, &[remove("/Left/.ds_store")], Path::new(".")),
		Err(OverlayError::FinderStore { .. })
	));
	assert!(matches!(
		apply(
			&mut image,
			&[position("/Left", FieldChange::Set([1.5, 2.0]))],
			Path::new(".")
		),
		Err(OverlayError::FinderPosition { index: 0 })
	));
	assert_eq!(store_bytes(&image, "/Left/.DS_Store"), bytes);
}
