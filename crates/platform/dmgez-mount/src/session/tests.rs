use std::{
	fs,
	os::unix::fs::{PermissionsExt, symlink},
	path::PathBuf,
};

use serde_json::{Value, json};

use super::*;

const PRIVATE_DIRECTORY: u32 = 0o700;
const PRIVATE_FILE: u32 = 0o600;
const PERMISSIONS_MASK: u32 = 0o777;
const MOCK_DEVICE: &str = "/dev/disk42";

struct Fixture {
	_temporary: tempfile::TempDir,
	directory: PathBuf,
}

impl Fixture {
	fn new() -> Self {
		let temporary = tempfile::tempdir().unwrap();
		Self {
			directory: temporary.path().canonicalize().unwrap(),
			_temporary: temporary,
		}
	}

	fn root(&self) -> PathBuf {
		self.directory.join("registry")
	}

	fn image(&self) -> PathBuf {
		self.directory.join("source.dmg")
	}

	fn record_file(&self, record: &SessionRecord) -> PathBuf {
		self.root().join(record.id().as_str()).join(wire::RECORD)
	}

	fn start(&self, store: &SessionStore) -> SessionRecord {
		store.start(&self.image(), None).unwrap()
	}
}

fn mode(path: &Path) -> u32 {
	fs::symlink_metadata(path).unwrap().permissions().mode() & PERMISSIONS_MASK
}

fn mock_attachment(record: &SessionRecord) -> Value {
	let timestamp = json!({ "seconds": 1, "nanos": 2 });
	let identity = json!({ "device": 80, "inode": 4, "created": timestamp });
	json!({
		"image": record.image(),
		"mountpoint": record.mountpoint(),
		"device": MOCK_DEVICE,
		"source": { "id": identity, "length": 512, "modified": timestamp, "changed": timestamp },
		"underlying": identity,
		"mount": { "root": identity, "fsid": 6 },
		"node": { "id": identity, "rdev": 7, "changed": timestamp },
	})
}

#[test]
fn records_and_managed_mountpoints_survive_reopening_in_id_order() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	let first = fixture.start(&store);
	let second = fixture.start(&store);
	assert_ne!(first.id(), second.id());
	let mut expected = vec![first.id().clone(), second.id().clone()];
	expected.sort_unstable();
	assert_eq!(mode(&fixture.root()), PRIVATE_DIRECTORY);
	assert_eq!(mode(&fixture.record_file(&first)), PRIVATE_FILE);
	assert_eq!(mode(first.mountpoint()), PRIVATE_DIRECTORY);
	assert_eq!(
		first.mountpoint(),
		fixture
			.root()
			.join(first.id().as_str())
			.join(OWNED_MOUNTPOINT)
	);
	assert!(fs::read_dir(first.mountpoint()).unwrap().next().is_none());
	drop(store);

	let store = SessionStore::open(&fixture.root()).unwrap();
	let loaded = store.get(first.id()).unwrap();
	assert_eq!(loaded.image(), fixture.image());
	assert_eq!(loaded.mountpoint(), first.mountpoint());
	assert!(loaded.managed_mountpoint());
	assert!(matches!(loaded.phase(), Phase::Starting));
	assert!(loaded.last_attachment().is_none());
	let actual: Vec<_> = store
		.list()
		.unwrap()
		.into_iter()
		.map(|item| item.id)
		.collect();
	assert_eq!(actual, expected);
}

#[test]
fn keeps_one_exclusive_store_lock_and_checks_the_named_root() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	assert!(matches!(
		SessionStore::open(&fixture.root()),
		Err(Error::Busy)
	));
	let held_path = fixture.directory.join("old-registry");
	fs::rename(fixture.root(), &held_path).unwrap();
	fs::create_dir(fixture.root()).unwrap();
	fs::set_permissions(
		fixture.root(),
		fs::Permissions::from_mode(PRIVATE_DIRECTORY),
	)
	.unwrap();
	assert!(matches!(store.list(), Err(Error::RegistryChanged)));
	drop(store);
	assert!(SessionStore::open(&fixture.root()).is_ok());
}

#[test]
fn refuses_a_root_alias_and_a_nonprivate_root_without_modifying_them() {
	let fixture = Fixture::new();
	let outside = fixture.directory.join("outside");
	fs::create_dir(&outside).unwrap();
	fs::set_permissions(&outside, fs::Permissions::from_mode(PRIVATE_DIRECTORY)).unwrap();
	symlink(&outside, fixture.root()).unwrap();
	assert!(SessionStore::open(&fixture.root()).is_err());
	assert!(fs::read_dir(&outside).unwrap().next().is_none());
	assert!(fs::symlink_metadata(fixture.root()).unwrap().is_symlink());
	fs::remove_file(fixture.root()).unwrap();

	const PUBLIC_DIRECTORY: u32 = 0o755;
	fs::create_dir(fixture.root()).unwrap();
	fs::set_permissions(fixture.root(), fs::Permissions::from_mode(PUBLIC_DIRECTORY)).unwrap();
	assert!(matches!(
		SessionStore::open(&fixture.root()),
		Err(Error::Privacy)
	));
	assert_eq!(mode(&fixture.root()), PUBLIC_DIRECTORY);
}

#[test]
fn rejects_ambiguous_paths_and_names_before_allocating_a_session() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	for invalid in ["", "..", "../session", "/dev/disk1", "mount-abc"] {
		assert!(matches!(
			invalid.parse::<SessionId>(),
			Err(Error::InvalidId)
		));
	}
	assert!(matches!(
		store.start(Path::new("relative.dmg"), None),
		Err(Error::Path)
	));
	assert!(matches!(
		store.start(&fixture.directory.join("../other.dmg"), None),
		Err(Error::Path)
	));
	assert!(matches!(
		store.start(&fixture.image(), Some(&fixture.root())),
		Err(Error::RegistryOverlap)
	));
	assert!(matches!(
		store.start(&fixture.image(), Some(&fixture.directory)),
		Err(Error::RegistryOverlap)
	));
	assert!(store.list().unwrap().is_empty());
}

#[test]
fn follows_only_allowed_transitions_and_retains_confirmed_evidence() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	let mut record = fixture.start(&store);
	let mut stale = record.clone();

	let mut wrong = mock_attachment(&record);
	wrong["image"] = json!(fixture.directory.join("other.dmg"));
	let wrong = serde_json::from_value::<Attachment>(wrong).unwrap();
	assert!(matches!(
		store.mark_mounted(&mut record, wrong),
		Err(Error::InvalidRecord)
	));
	assert!(matches!(
		store.get(record.id()).unwrap().phase(),
		Phase::Starting
	));

	let correct = serde_json::from_value::<Attachment>(mock_attachment(&record)).unwrap();
	store.mark_mounted(&mut record, correct).unwrap();
	assert!(matches!(record.phase(), Phase::Mounted(_)));
	assert!(matches!(
		store.mark_detached(&mut stale),
		Err(Error::RecordChanged)
	));
	store.mark_uncertain(&mut record).unwrap();
	assert!(matches!(record.phase(), Phase::Uncertain));
	assert_eq!(record.last_attachment().unwrap().device(), MOCK_DEVICE);
	store.mark_detached(&mut record).unwrap();
	let loaded = store.get(record.id()).unwrap();
	assert!(matches!(loaded.phase(), Phase::Detached));
	assert_eq!(loaded.last_attachment().unwrap().device(), MOCK_DEVICE);
	assert!(matches!(
		store.mark_uncertain(&mut record),
		Err(Error::Transition)
	));
}

#[test]
fn explicit_and_managed_mountpoints_are_never_removed_by_record_updates() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	let explicit = fixture.directory.join("mount");
	fs::create_dir(&explicit).unwrap();
	let mut chosen = store.start(&fixture.image(), Some(&explicit)).unwrap();
	let mut managed = fixture.start(&store);
	for record in [&mut chosen, &mut managed] {
		let marker = record.mountpoint().join("untouched");
		fs::write(&marker, b"retained").unwrap();
		store.mark_detached(record).unwrap();
		assert_eq!(fs::read(marker).unwrap(), b"retained");
	}
	assert!(!chosen.managed_mountpoint());
	assert!(managed.managed_mountpoint());
}

#[test]
fn retires_only_the_regular_private_pending_file_after_an_interrupted_update() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	let mut record = fixture.start(&store);
	let path = fixture.record_file(&record);
	let staged = path.with_file_name(wire::PENDING_RECORD);
	fs::write(&staged, b"incomplete").unwrap();
	fs::set_permissions(&staged, fs::Permissions::from_mode(PRIVATE_FILE)).unwrap();
	assert!(matches!(
		store.get(record.id()).unwrap().phase(),
		Phase::Starting
	));
	store.mark_uncertain(&mut record).unwrap();
	assert!(!staged.exists());
	assert!(matches!(
		store.get(record.id()).unwrap().phase(),
		Phase::Uncertain
	));

	let outside = fixture.directory.join("persistent");
	fs::write(&outside, b"original").unwrap();
	symlink(&outside, &staged).unwrap();
	assert!(matches!(
		store.mark_detached(&mut record),
		Err(Error::Privacy)
	));
	assert_eq!(fs::read(&outside).unwrap(), b"original");
	assert!(fs::symlink_metadata(&staged).unwrap().is_symlink());
	assert!(matches!(
		store.get(record.id()).unwrap().phase(),
		Phase::Uncertain
	));
}

#[test]
fn refuses_oversized_records_and_multiply_linked_records() {
	let fixture = Fixture::new();
	let store = SessionStore::open(&fixture.root()).unwrap();
	let record = fixture.start(&store);
	let path = fixture.record_file(&record);
	let alias = fixture.directory.join("outside-link");
	fs::hard_link(&path, &alias).unwrap();
	assert!(matches!(store.get(record.id()), Err(Error::Privacy)));
	assert!(alias.exists());
	fs::remove_file(alias).unwrap();

	let file = fs::OpenOptions::new().write(true).open(path).unwrap();
	file.set_len(wire::MAX_RECORD_BYTES as u64 + 1).unwrap();
	assert!(matches!(store.get(record.id()), Err(Error::Limit)));
}
