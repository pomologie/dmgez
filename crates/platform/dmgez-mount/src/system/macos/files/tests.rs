use std::{fs, os::unix::fs::symlink};

use super::{HeldDirectory, HeldImage, image_alias, named_image_matches};
use crate::system::Error;

#[test]
fn empty_mountpoint_requires_the_same_named_directory_without_symlinks() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let selected = root.join("mount");
	let renamed = root.join("old");
	let alias = root.join("alias");
	fs::create_dir(&selected).unwrap();
	let held = HeldDirectory::open(&selected).unwrap();
	held.verify_empty(&selected).unwrap();

	fs::write(selected.join("entry"), []).unwrap();
	assert!(matches!(
		held.verify_empty(&selected),
		Err(Error::MountpointOccupied { .. })
	));
	fs::remove_file(selected.join("entry")).unwrap();

	fs::rename(&selected, &renamed).unwrap();
	fs::create_dir(&selected).unwrap();
	assert!(matches!(
		held.verify_empty(&selected),
		Err(Error::IdentityMismatch)
	));

	symlink(&renamed, &alias).unwrap();
	assert!(HeldDirectory::open(&alias).is_err());
}

#[test]
fn held_image_rejects_changes_and_detects_existing_hard_link_aliases() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let selected = root.join("image.dmg");
	let linked = root.join("linked.dmg");
	let symlinked = root.join("alias.dmg");
	fs::write(&selected, []).unwrap();
	fs::hard_link(&selected, &linked).unwrap();
	symlink(&selected, &symlinked).unwrap();

	let held = HeldImage::open(&selected).unwrap();
	held.verify(&selected).unwrap();
	assert!(image_alias(&linked, held.stamp.id));
	assert!(HeldImage::open(&symlinked).is_err());

	fs::write(&linked, [1]).unwrap();
	assert!(matches!(
		held.verify(&selected),
		Err(Error::IdentityMismatch)
	));
	assert!(!named_image_matches(&selected, held.stamp).unwrap());
}

#[test]
fn held_image_rejects_an_unrecognized_container_before_attachment() {
	let temp = tempfile::tempdir().unwrap();
	let image = temp.path().canonicalize().unwrap().join("unrecognized.dmg");
	fs::write(&image, []).unwrap();
	let held = HeldImage::open(&image).unwrap();
	assert!(matches!(
		held.admit_format(&image),
		Err(Error::UnsupportedImage)
	));
}
