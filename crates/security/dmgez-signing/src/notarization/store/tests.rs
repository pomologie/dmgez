use std::{
	fs::{self, File, OpenOptions},
	io::Write,
	os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
	path::Path,
	process::Command,
	thread,
};

use rustix::fs::{FlockOperation, flock};

use super::{Error, LOCAL_LOCK_PAUSE, MAX_RECORD_BYTES, RECORD_MODE, Store};
use crate::notarization::{Report, Status};

const RECORD: &str = "notarization.json";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ARTIFACT_BYTES: u64 = 4_096;
const CREATED_AT: u64 = 1_790_000_000;
const SUBMISSION_ID: &str = "77f9ad63-3d09-4dc6-8ee8-a63b57e5feb1";
const UNRELATED_DATA: &[u8] = b"unrelated record";
const CHMOD: &str = "/bin/chmod";

#[test]
fn reserves_a_private_record_without_clobbering_an_existing_one() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	let expected = reservation();
	let _store = Store::create(&path, &expected).unwrap();

	let file = fs::metadata(&path).unwrap();
	assert_eq!(file.permissions().mode() & 0o777, record_mode());
	assert_eq!(Store::open(&path).unwrap().1, expected);
	assert!(matches!(
		Store::create(&path, &pending()),
		Err(Error::Exists)
	));
	assert_eq!(Store::open(&path).unwrap().1, expected);
}

#[test]
fn rejects_stale_replacement_and_retains_the_first_update() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	let mut store = Store::create(&path, &reservation()).unwrap();
	let (mut stale, _) = Store::open(&path).unwrap();
	let expected = pending();
	store.replace(&expected).unwrap();

	assert!(matches!(stale.replace(&reservation()), Err(Error::Changed)));
	assert_eq!(Store::open(&path).unwrap().1, expected);
	store.replace(&expected).unwrap();
}

#[test]
fn replaces_after_a_cooperating_writer_releases_the_lock() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	let mut store = Store::create(&path, &reservation()).unwrap();
	let competing_directory = File::open(folder.path()).unwrap();
	flock(
		&competing_directory,
		FlockOperation::NonBlockingLockExclusive,
	)
	.unwrap();
	let release = thread::spawn(move || {
		thread::sleep(4 * LOCAL_LOCK_PAUSE);
		flock(competing_directory, FlockOperation::Unlock).unwrap();
	});

	let expected = pending();
	store.replace(&expected).unwrap();
	release.join().unwrap();
	assert_eq!(Store::open(&path).unwrap().1, expected);
}

#[test]
fn refuses_public_and_aliased_records_before_reading_them() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	let target = folder.path().join("target.json");
	private_file(&target).write_all(UNRELATED_DATA).unwrap();
	symlink(&target, &path).unwrap();

	assert!(matches!(Store::open(&path), Err(Error::PrivateFile)));
	assert!(matches!(
		Store::create(&path, &reservation()),
		Err(Error::Exists)
	));
	assert_eq!(fs::read(&target).unwrap(), UNRELATED_DATA);
	fs::remove_file(&path).unwrap();
	fs::hard_link(&target, &path).unwrap();
	assert!(matches!(Store::open(&path), Err(Error::PrivateFile)));
	fs::remove_file(&path).unwrap();

	let _store = Store::create(&path, &reservation()).unwrap();
	fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
	assert!(matches!(Store::open(&path), Err(Error::PrivateFile)));
}

#[test]
fn validates_record_semantics_on_read_create_and_replace() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	let mut invalid = reservation();
	invalid.status = Status::Accepted;

	assert!(matches!(
		Store::create(&path, &invalid),
		Err(Error::Invalid)
	));
	assert!(!path.exists());
	let mut store = Store::create(&path, &reservation()).unwrap();
	assert!(matches!(store.replace(&invalid), Err(Error::Invalid)));
	assert_eq!(Store::open(&path).unwrap().1, reservation());

	private_file(&path)
		.write_all(&serde_json::to_vec(&invalid).unwrap())
		.unwrap();
	assert!(matches!(Store::open(&path), Err(Error::Invalid)));
}

#[test]
fn bounds_record_reads() {
	let folder = tempfile::tempdir().unwrap();
	let path = folder.path().join(RECORD);
	private_file(&path).set_len(MAX_RECORD_BYTES + 1).unwrap();

	assert!(matches!(Store::open(&path), Err(Error::Size)));
}

#[test]
fn refuses_parent_and_leaf_access_control_lists() {
	let folder = tempfile::tempdir().unwrap();
	let parent = folder.path().join("records");
	fs::create_dir(&parent).unwrap();
	let path = parent.join(RECORD);
	add_acl(
		&parent,
		"everyone allow list,file_inherit,directory_inherit",
	);
	assert!(matches!(
		Store::create(&path, &reservation()),
		Err(Error::AccessControl)
	));
	assert!(fs::read_dir(&parent).unwrap().next().is_none());

	let path = folder.path().join(RECORD);
	let _store = Store::create(&path, &reservation()).unwrap();
	add_acl(&path, "everyone allow read");
	assert_eq!(
		fs::metadata(&path).unwrap().permissions().mode() & 0o777,
		record_mode()
	);
	assert!(matches!(Store::open(&path), Err(Error::AccessControl)));
}

fn add_acl(path: &Path, entry: &str) {
	let output = Command::new(CHMOD)
		.args(["+a", entry])
		.arg(path)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
}

#[test]
fn refuses_to_publish_into_a_replaced_parent() {
	let folder = tempfile::tempdir().unwrap();
	let parent = folder.path().join("records");
	let moved = folder.path().join("moved");
	fs::create_dir(&parent).unwrap();
	let path = parent.join(RECORD);
	let mut store = Store::create(&path, &reservation()).unwrap();
	fs::rename(&parent, &moved).unwrap();
	fs::create_dir(&parent).unwrap();
	private_file(&path).write_all(UNRELATED_DATA).unwrap();

	assert!(matches!(store.replace(&pending()), Err(Error::Changed)));
	assert_eq!(fs::read(&path).unwrap(), UNRELATED_DATA);
	assert_eq!(Store::open(&moved.join(RECORD)).unwrap().1, reservation());
}

fn reservation() -> Report {
	Report::reserve(DIGEST.to_owned(), ARTIFACT_BYTES, CREATED_AT)
}

fn pending() -> Report {
	Report {
		submission_id: Some(SUBMISSION_ID.parse().unwrap()),
		status: Status::Pending,
		updated_at: CREATED_AT + 1,
		issue: None,
		..reservation()
	}
}

fn record_mode() -> u32 {
	u32::from(RECORD_MODE.as_raw_mode())
}

fn private_file(path: &Path) -> File {
	OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(record_mode())
		.open(path)
		.unwrap()
}
