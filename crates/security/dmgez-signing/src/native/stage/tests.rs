use std::{
	io::Write,
	os::unix::{fs::PermissionsExt, fs::symlink},
};

use super::*;

const OUTPUT_NAME: &str = "signed.dmg";
const STAGED_CONTENTS: &[u8] = b"verified bytes";
const EXISTING_CONTENTS: &[u8] = b"existing bytes";
const OWNER_ACCESS_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o777;

fn staged_image(stage: &Stage) -> File {
	let mut image = stage.create_image().unwrap();
	image.write_all(STAGED_CONTENTS).unwrap();
	image
}

#[test]
fn publishes_verified_handle_and_removes_private_stage() {
	let directory = tempfile::tempdir().unwrap();
	let destination = directory.path().join(OUTPUT_NAME);
	let stage = Stage::new(&destination).unwrap();
	let stage_parent = stage.path().parent().unwrap().to_path_buf();
	let stage_mode = fs::metadata(&stage_parent).unwrap().permissions().mode();
	assert_eq!(stage_mode & PERMISSION_BITS, OWNER_ACCESS_MODE);
	drop(staged_image(&stage));
	let verified = File::open(stage.path()).unwrap();

	let published = stage.publish(&verified).unwrap();

	assert_eq!(published, destination.canonicalize().unwrap());
	assert_eq!(fs::read(destination).unwrap(), STAGED_CONTENTS);
	assert!(!stage_parent.exists());
}

#[test]
fn refuses_existing_or_syntactically_nonfile_destinations() {
	let directory = tempfile::tempdir().unwrap();
	let output = directory.path().join(OUTPUT_NAME);
	symlink(directory.path().join("missing"), &output).unwrap();
	assert!(matches!(Stage::new(&output), Err(Error::AlreadyExists)));
	assert!(fs::symlink_metadata(&output).unwrap().is_symlink());
	for name in ["signed.dmg/", "signed.dmg/.", "signed.dmg/..", "signed.img"] {
		assert!(matches!(
			Stage::new(&directory.path().join(name)),
			Err(Error::InvalidDestination)
		));
	}
}

#[test]
fn a_competing_output_survives_and_the_stage_is_cleaned() {
	let directory = tempfile::tempdir().unwrap();
	let destination = directory.path().join(OUTPUT_NAME);
	let stage = Stage::new(&destination).unwrap();
	let stage_parent = stage.path().parent().unwrap().to_path_buf();
	let verified = staged_image(&stage);
	fs::write(&destination, EXISTING_CONTENTS).unwrap();

	assert!(matches!(
		stage.publish(&verified),
		Err(Error::AlreadyExists)
	));

	assert_eq!(fs::read(destination).unwrap(), EXISTING_CONTENTS);
	assert!(!stage_parent.exists());
}

#[test]
fn a_replaced_image_cannot_be_published_with_the_verified_handle() {
	let directory = tempfile::tempdir().unwrap();
	let destination = directory.path().join(OUTPUT_NAME);
	let stage = Stage::new(&destination).unwrap();
	let verified = staged_image(&stage);
	let unrelated = directory.path().join("outside");
	fs::write(&unrelated, EXISTING_CONTENTS).unwrap();
	fs::remove_file(stage.path()).unwrap();
	symlink(&unrelated, stage.path()).unwrap();

	assert!(matches!(stage.publish(&verified), Err(Error::StageChanged)));
	assert!(!destination.exists());
	assert_eq!(fs::read(unrelated).unwrap(), EXISTING_CONTENTS);
}

#[test]
fn the_output_parent_must_still_name_the_anchored_directory() {
	let directory = tempfile::tempdir().unwrap();
	let original = directory.path().join("original");
	let moved = directory.path().join("moved");
	fs::create_dir(&original).unwrap();
	let stage = Stage::new(&original.join(OUTPUT_NAME)).unwrap();
	let stage_name = stage.path().parent().unwrap().file_name().unwrap();
	let replacement = original.join(stage_name);
	let verified = staged_image(&stage);
	fs::rename(&original, &moved).unwrap();
	fs::create_dir(&original).unwrap();
	fs::create_dir(&replacement).unwrap();
	fs::write(replacement.join(STAGED_IMAGE), EXISTING_CONTENTS).unwrap();

	assert!(matches!(
		stage.publish(&verified),
		Err(Error::DirectoryChanged)
	));

	assert_eq!(
		fs::read(replacement.join(STAGED_IMAGE)).unwrap(),
		EXISTING_CONTENTS
	);
	assert!(!moved.join(OUTPUT_NAME).exists());
	assert_eq!(fs::read_dir(moved).unwrap().count(), 0);
}
