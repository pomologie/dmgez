use std::{
	io::Write,
	os::unix::{fs::PermissionsExt, fs::symlink},
};

use super::*;

const OUTPUT_NAME: &str = "signed.dmg";
const OUTPUT_APP_NAME: &str = "signed.app";
const SOURCE_APP_NAME: &str = "source.app";
const BUNDLE_CONTENTS: &str = "Contents";
const RESOURCE: &str = "resource";
const STAGED_CONTENTS: &[u8] = b"verified bytes";
const EXISTING_CONTENTS: &[u8] = b"existing bytes";
const OWNER_ACCESS_MODE: u32 = 0o700;
const PERMISSION_BITS: u32 = 0o777;
const READ_ONLY_DIRECTORY_MODE: u32 = 0o555;

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

fn staged_app(stage: &Stage) -> File {
	assert!(!stage.path().exists());
	fs::create_dir_all(stage.path().join(BUNDLE_CONTENTS)).unwrap();
	fs::write(
		stage.path().join(BUNDLE_CONTENTS).join(RESOURCE),
		STAGED_CONTENTS,
	)
	.unwrap();
	let verified = File::open(stage.path()).unwrap();
	stage.validate_app(&verified).unwrap();
	verified
}

#[test]
fn publishes_verified_app_to_a_new_path() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_APP_NAME);
	let output = directory.path().join(OUTPUT_APP_NAME);
	fs::create_dir(&source).unwrap();
	let stage = Stage::new_app(&output, &source).unwrap();
	let stage_parent = stage.path().parent().unwrap().to_path_buf();
	let verified = staged_app(&stage);

	let published = stage.publish_app(&verified).unwrap();

	assert_eq!(published, output.canonicalize().unwrap());
	assert_eq!(
		fs::read(output.join(BUNDLE_CONTENTS).join(RESOURCE)).unwrap(),
		STAGED_CONTENTS
	);
	assert_eq!(fs::read_dir(source).unwrap().count(), 0);
	assert!(!stage_parent.exists());
}

#[test]
fn rejects_app_output_parents_inside_the_source_even_through_aliases() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_APP_NAME);
	let contents = source.join(BUNDLE_CONTENTS);
	let alias = directory.path().join("alias");
	fs::create_dir_all(&contents).unwrap();
	symlink(&contents, &alias).unwrap();
	for parent in [&source, &contents, &alias] {
		assert!(matches!(
			Stage::new_app(&parent.join(OUTPUT_APP_NAME), &source),
			Err(Error::InsideSource)
		));
	}
	assert_eq!(fs::read_dir(source).unwrap().count(), 1);
	assert_eq!(fs::read_dir(contents).unwrap().count(), 0);
}

#[test]
fn competing_app_and_external_link_target_survive_failed_publication() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_APP_NAME);
	let output = directory.path().join(OUTPUT_APP_NAME);
	fs::create_dir(&source).unwrap();
	let original_resource = source.join(RESOURCE);
	fs::write(&original_resource, EXISTING_CONTENTS).unwrap();
	let stage = Stage::new_app(&output, &source).unwrap();
	let stage_parent = stage.path().parent().unwrap().to_path_buf();
	let verified = staged_app(&stage);
	let contents = stage.path().join(BUNDLE_CONTENTS);
	symlink(&source, contents.join("link")).unwrap();
	fs::set_permissions(
		contents,
		fs::Permissions::from_mode(READ_ONLY_DIRECTORY_MODE),
	)
	.unwrap();
	fs::create_dir(&output).unwrap();
	fs::write(output.join(RESOURCE), EXISTING_CONTENTS).unwrap();

	assert!(matches!(
		stage.publish_app(&verified),
		Err(Error::AlreadyExists)
	));
	assert_eq!(fs::read(original_resource).unwrap(), EXISTING_CONTENTS);
	assert_eq!(fs::read(output.join(RESOURCE)).unwrap(), EXISTING_CONTENTS);
	assert!(!stage_parent.exists());
}

#[test]
fn invalid_app_root_replacement_is_never_recursively_removed() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_APP_NAME);
	let output = directory.path().join(OUTPUT_APP_NAME);
	let moved = directory.path().join("moved.app");
	fs::create_dir(&source).unwrap();
	let stage = Stage::new_app(&output, &source).unwrap();
	let stage_path = stage.path().to_path_buf();
	let verified = staged_app(&stage);
	fs::rename(&stage_path, &moved).unwrap();
	fs::create_dir(&stage_path).unwrap();
	let unrelated = stage_path.join(RESOURCE);
	fs::write(&unrelated, EXISTING_CONTENTS).unwrap();
	let replacement = File::open(&stage_path).unwrap();

	assert!(matches!(
		stage.validate_app(&replacement),
		Err(Error::StageChanged)
	));
	assert!(matches!(
		stage.publish_app(&verified),
		Err(Error::StageChanged)
	));
	assert_eq!(fs::read(unrelated).unwrap(), EXISTING_CONTENTS);
	assert_eq!(
		fs::read(moved.join(BUNDLE_CONTENTS).join(RESOURCE)).unwrap(),
		STAGED_CONTENTS
	);
	assert!(!output.exists());
}
