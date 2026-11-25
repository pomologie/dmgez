use super::*;
use dmgez_config::project_edit::ContentChanges;

type TestSnapshot = Snapshot<'static, &'static [u8]>;

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
fn refuses_unqualified_finder_and_copy_operations() {
	let mut image = snapshot();
	assert!(matches!(
		apply(
			&mut image,
			&[add_directory("/Right/.ds_store"),],
			Path::new(".")
		),
		Err(OverlayError::FinderStore { .. })
	));
	let mut changes = ContentChanges::default();
	changes.hidden = FieldChange::Set(true);
	assert!(matches!(
		apply(
			&mut image,
			&[ProjectEdit::Modify {
				target: path("/Right"),
				changes
			},],
			Path::new(".")
		),
		Err(OverlayError::FinderChange { index: 0 })
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
fn a_directory_move_cannot_push_descendants_past_the_depth_budget() {
	let mut image = snapshot();
	let destination = "/parent".repeat(MAX_DEPTH);
	assert!(matches!(
		apply(&mut image, &[rename("/Left", &destination)], Path::new(".")),
		Err(OverlayError::Budget)
	));
	assert_eq!(target(&image, "/Left/E\u{301}"), b"hard\xfftarget");
}
