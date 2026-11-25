use std::{
	fs::File,
	os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

use rustix::fs::{XattrFlags, fgetxattr_query, fsetxattr};

use super::*;

const SOURCE: &str = "Example.app";
const STAGED: &str = "Stage.app";
const LAYOUT: &str = "Contents/Frameworks/Example.framework";
const VERSION: &str = "Versions/A";
const BINARY: &str = "Versions/A/Example";
const ALIAS: &str = "Versions/A/Second";
const RESOURCE_FORK: &[u8] = b"com.apple.ResourceFork";
const XATTR: &[u8] = b"org.dmgez.fixture";
const QUARANTINE: &[u8] = b"com.apple.quarantine";

#[test]
fn copies_framework_links_forks_modes_and_quarantine_without_aliasing_the_source() {
	let root = parent();
	let source = root.path().join(SOURCE);
	let staged = root.path().join(STAGED);
	let framework = source.join(LAYOUT);
	fs::create_dir_all(framework.join(VERSION)).unwrap();
	let binary = framework.join(BINARY);
	fs::write(&binary, b"executable payload").unwrap();
	fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
	fs::hard_link(&binary, framework.join(ALIAS)).unwrap();
	symlink("A", framework.join("Versions/Current")).unwrap();
	symlink("Versions/Current/Example", framework.join("Example")).unwrap();
	let file = File::open(&binary).unwrap();
	fsetxattr(
		&file,
		RESOURCE_FORK,
		b"resource payload",
		XattrFlags::empty(),
	)
	.unwrap();
	fsetxattr(&file, XATTR, b"fixture attribute", XattrFlags::empty()).unwrap();
	let bundle = File::open(&source).unwrap();
	fsetxattr(
		&bundle,
		QUARANTINE,
		b"0083;00000000;dmgez;",
		XattrFlags::empty(),
	)
	.unwrap();

	let snapshot = copy_bundle(&source, &staged).unwrap();
	snapshot.validate_source().unwrap();
	let staged_framework = staged.join(LAYOUT);
	let staged_binary = staged_framework.join(BINARY);
	let staged_file = fs::metadata(&staged_binary).unwrap();
	assert_ne!(staged_file.ino(), file.metadata().unwrap().ino());
	assert_eq!(
		staged_file.ino(),
		fs::metadata(staged_framework.join(ALIAS)).unwrap().ino()
	);
	assert_eq!(
		fs::read_link(staged_framework.join("Example")).unwrap(),
		Path::new("Versions/Current/Example")
	);
	assert!(fgetxattr_query(File::open(staged).unwrap(), QUARANTINE).unwrap() > 0);

	fs::write(&binary, b"changed original").unwrap();
	assert!(matches!(
		snapshot.validate_source(),
		Err(CopyError::SourceChanged(_))
	));
	assert_eq!(fs::read(staged_binary).unwrap(), b"executable payload");
}

#[test]
fn refuses_missing_cyclic_and_external_links_before_copying() {
	let parent = parent();
	let source = parent.path().join(SOURCE);
	let staged = parent.path().join(STAGED);
	fs::create_dir(&source).unwrap();
	fs::write(parent.path().join("external"), b"untouched").unwrap();
	let link = source.join("link");
	for target in ["missing", "link", "../external"] {
		symlink(target, &link).unwrap();
		assert!(matches!(
			copy_bundle(&source, &staged),
			Err(CopyError::Link(_))
		));
		assert!(!staged.exists());
		fs::remove_file(&link).unwrap();
	}
	assert_eq!(
		fs::read(parent.path().join("external")).unwrap(),
		b"untouched"
	);
}

#[test]
fn refuses_an_existing_stage_without_changing_it() {
	let parent = parent();
	let source = parent.path().join(SOURCE);
	let staged = parent.path().join(STAGED);
	fs::create_dir(&source).unwrap();
	fs::write(&staged, b"previous output").unwrap();
	assert!(matches!(
		copy_bundle(&source, &staged),
		Err(CopyError::Exists)
	));
	assert_eq!(fs::read(staged).unwrap(), b"previous output");
}

#[test]
fn identical_bytes_do_not_admit_a_stage_hard_link_to_the_source() {
	let parent = parent();
	let source = parent.path().join(SOURCE);
	let staged = parent.path().join(STAGED);
	const PAYLOAD: &str = "Payload";
	fs::create_dir(&source).unwrap();
	fs::create_dir(&staged).unwrap();
	fs::write(source.join(PAYLOAD), b"same payload").unwrap();
	fs::hard_link(source.join(PAYLOAD), staged.join(PAYLOAD)).unwrap();
	let originals = Inventory::read(&source, Inspection::Full).unwrap();
	let copies = Inventory::read(&staged, Inspection::Full).unwrap();

	assert!(matches!(
		originals.compare_copy(&copies),
		Err(CopyError::CopyDiffers(path)) if path == Path::new(PAYLOAD)
	));
}

fn parent() -> tempfile::TempDir {
	// macOS represents /var and /tmp as links. The source and stage owners
	// hand the copy tool canonical absolute paths.
	let temp = std::env::temp_dir().canonicalize().unwrap();
	tempfile::tempdir_in(temp).unwrap()
}
