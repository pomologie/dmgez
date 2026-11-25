//! Host copy discovery for image edits (D-OPS-01).

#![cfg(any(unix, windows))]

use std::{fs, io::Read};

use dmgez_builder::manifest::{Budget, CopyInput, EntryKind, Error, Limits, discover_copies};
use dmgez_config::{ContentError, VirtualPath};

#[test]
fn captures_recursive_sources_and_literal_default_names() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let batch = root.join("batch");
	fs::create_dir_all(batch.join("tree/nested")).unwrap();
	fs::write(batch.join("tree/nested/payload.txt"), b"replacement").unwrap();
	fs::hard_link(
		batch.join("tree/nested/payload.txt"),
		batch.join("tree/alias.txt"),
	)
	.unwrap();
	fs::write(batch.join("${LITERAL}.txt"), b"literal filename").unwrap();
	let destination = "/Existing/Folder".parse().unwrap();
	let manifest = discover_copies(
		&[
			CopyInput {
				source: "tree",
				destination: Some(&destination),
			},
			CopyInput {
				source: "${LITERAL}.txt",
				destination: None,
			},
		],
		&batch,
		&root.join("edited.dmg"),
		Limits::default(),
	)
	.unwrap();
	let entries = manifest.entries();
	assert_eq!(
		entries
			.iter()
			.map(|entry| (entry.destination.as_str(), entry.source.is_some()))
			.collect::<Vec<_>>(),
		[
			("/${LITERAL}.txt", true),
			("/Existing", false),
			("/Existing/Folder", true),
			("/Existing/Folder/alias.txt", true),
			("/Existing/Folder/nested", true),
			("/Existing/Folder/nested/payload.txt", true),
		]
	);
	assert!(matches!(entries[2].kind, EntryKind::Directory));
	let source = entries[5].source.as_ref().unwrap();
	let linked = entries[3].source.as_ref().unwrap();
	assert_eq!(source.metadata().identity, linked.metadata().identity);
	let mut contents = Vec::new();
	source
		.open_regular()
		.unwrap()
		.read_to_end(&mut contents)
		.unwrap();
	assert_eq!(contents, b"replacement");
	manifest.validate_sources().unwrap();
	fs::rename(
		batch.join("tree/nested/payload.txt"),
		root.join("moved.txt"),
	)
	.unwrap();
	fs::write(batch.join("tree/nested/payload.txt"), b"replacement").unwrap();
	assert!(matches!(
		manifest.validate_sources(),
		Err(Error::SourceChanged { .. })
	));
}

#[test]
fn rejects_output_aliases_and_output_inside_a_copied_tree() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	fs::create_dir_all(root.join("tree/nested")).unwrap();
	fs::write(root.join("tree/nested/source.txt"), b"data").unwrap();
	fs::hard_link(root.join("tree/nested/source.txt"), root.join("alias.dmg")).unwrap();
	for output in [root.join("alias.dmg"), root.join("tree/nested/new.dmg")] {
		assert!(matches!(
			discover_copies(
				&[CopyInput {
					source: "tree",
					destination: None,
				}],
				&root,
				&output,
				Limits::default(),
			),
			Err(Error::OutputSourceAlias { .. })
		));
	}
}

#[cfg(target_os = "linux")]
#[test]
fn detects_aliases_at_output_paths_with_non_utf8_names() {
	use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let output = root.join(OsStr::from_bytes(b"output-\xff.dmg"));
	fs::write(root.join("source.txt"), b"data").unwrap();
	fs::hard_link(root.join("source.txt"), &output).unwrap();
	assert!(matches!(
		discover_copies(
			&[CopyInput {
				source: "source.txt",
				destination: None,
			}],
			&root,
			&output,
			Limits::default(),
		),
		Err(Error::OutputSourceAlias { .. })
	));
}

#[test]
fn rejects_nonlocal_sources_reserved_destinations_and_exhausted_walks() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let output = root.join("edited.dmg");
	let reserved: VirtualPath = "/.DS_store/nested".parse().unwrap();
	for (source, destination, expected) in [
		(
			"https://example.invalid/document",
			None,
			ContentError::InvalidSource,
		),
		("tree", Some(&reserved), ContentError::ReservedDestination),
	] {
		assert!(matches!(
			discover_copies(
				&[CopyInput { source, destination }],
				&root,
				&output,
				Limits::default(),
			),
			Err(Error::InvalidCopyInput { index: 0, source }) if source == expected
		));
	}
	fs::create_dir(root.join("tree")).unwrap();
	fs::write(root.join("tree/source.txt"), b"data").unwrap();
	assert!(matches!(
		discover_copies(
			&[CopyInput {
				source: "tree",
				destination: None,
			}],
			&root,
			&output,
			Limits {
				max_visited: 1,
				..Limits::default()
			},
		),
		Err(Error::Limit(Budget::Visited))
	));
}

#[cfg(unix)]
#[test]
fn retains_link_bytes_without_traversing_the_target() {
	use std::{
		ffi::OsStr,
		os::unix::{ffi::OsStrExt, fs::symlink},
	};

	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let target = b"../unavailable-\xff";
	symlink(OsStr::from_bytes(target), root.join("host-link")).unwrap();
	let manifest = discover_copies(
		&[CopyInput {
			source: "host-link",
			destination: None,
		}],
		&root,
		&root.join("edited.dmg"),
		Limits::default(),
	)
	.unwrap();
	let [entry] = manifest.entries() else {
		panic!("the symbolic link is the only requested entry");
	};
	assert!(matches!(&entry.kind, EntryKind::Symlink(bytes) if bytes == target));
	assert!(entry.source.is_some());
	manifest.validate_sources().unwrap();
}
