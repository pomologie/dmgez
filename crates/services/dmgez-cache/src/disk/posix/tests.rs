use std::{
	fs::{self, File, Permissions},
	os::unix::fs::{MetadataExt, PermissionsExt, symlink},
	path::{Path, PathBuf},
};

use rustix::fs::{self as anchored, Timespec, Timestamps};

use super::{
	Cache, Error,
	entry::{Name, PENDING},
};

const FIRST: [u8; 32] = [1; 32];
const SECOND: [u8; 32] = [2; 32];
const THIRD: [u8; 32] = [3; 32];
const DATA_BYTES: usize = 4;
const BLOCK: &[u8; DATA_BYTES] = b"data";
const UNKNOWN: &str = "unrelated";
const UNKNOWN_BYTES: &[u8] = b"user file";
const ACCESS_MODE_MASK: u32 = 0o777;
const OWNER_DIRECTORY_MODE: u32 = 0o700;
const OWNER_FILE_MODE: u32 = 0o600;
const PUBLIC_DIRECTORY_MODE: u32 = 0o755;

fn private_root() -> (tempfile::TempDir, PathBuf) {
	let temporary = tempfile::tempdir().unwrap();
	let path = temporary.path().canonicalize().unwrap();
	fs::set_permissions(&path, Permissions::from_mode(OWNER_DIRECTORY_MODE)).unwrap();
	(temporary, path)
}

fn open(path: &Path, cap: usize) -> Cache {
	Cache::open(path, (cap as u64).try_into().unwrap()).unwrap()
}

fn entry(path: &Path, key: &[u8; 32]) -> PathBuf {
	path.join(Name::for_key(key).as_os_str())
}

fn set_age(path: &Path, order: i64) {
	let file = File::open(path).unwrap();
	let age = Timespec {
		tv_sec: order,
		tv_nsec: 0,
	};
	anchored::futimens(
		&file,
		&Timestamps {
			last_access: age,
			last_modification: age,
		},
	)
	.unwrap();
}

#[test]
fn opens_without_mutating_and_holds_all_ancestor_identities() {
	let (_temporary, path) = private_root();
	let root_entry = entry(&path, &FIRST);
	fs::write(&root_entry, BLOCK).unwrap();
	let cache = open(&path, DATA_BYTES - 1);
	assert_eq!(fs::read(&root_entry).unwrap(), BLOCK);
	assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), None);
	let metadata = fs::metadata(&path).unwrap();
	assert_eq!(cache.directory_identity(), (metadata.dev(), metadata.ino()));
	let expected: Vec<_> = path
		.ancestors()
		.map(|path| {
			let metadata = fs::metadata(path).unwrap();
			(metadata.dev(), metadata.ino())
		})
		.collect();
	assert!(cache.ancestors().iter().rev().copied().eq(expected));
}

#[test]
fn published_blocks_are_private_persistent_and_bounded_by_the_read_buffer() {
	let (_temporary, path) = private_root();
	open(&path, DATA_BYTES).store(&FIRST, BLOCK);
	let file = entry(&path, &FIRST);
	assert_eq!(fs::read(&file).unwrap(), BLOCK);
	assert_eq!(
		fs::metadata(&file).unwrap().permissions().mode() & ACCESS_MODE_MASK,
		OWNER_FILE_MODE
	);
	let cache = open(&path, DATA_BYTES);
	let mut small = [0; DATA_BYTES - 1];
	assert_eq!(cache.load(&FIRST, &mut small), None);
	assert_eq!(small, [0; DATA_BYTES - 1]);
	let mut output = [0; DATA_BYTES + 1];
	assert_eq!(cache.load(&FIRST, &mut output), Some(DATA_BYTES));
	assert_eq!(&output[..DATA_BYTES], BLOCK);
	assert_eq!(output[DATA_BYTES], 0);
}

#[test]
fn abandoned_stage_is_ignored_until_a_store_replaces_it() {
	let (_temporary, path) = private_root();
	let pending = path.join(PENDING);
	fs::write(&pending, &BLOCK[..DATA_BYTES - 1]).unwrap();
	fs::set_permissions(&pending, Permissions::from_mode(OWNER_FILE_MODE)).unwrap();
	let cache = open(&path, DATA_BYTES);
	assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), None);
	assert!(pending.exists());
	cache.store(&FIRST, BLOCK);
	assert!(!pending.exists());
	assert_eq!(fs::read(entry(&path, &FIRST)).unwrap(), BLOCK);
}

#[test]
fn eviction_follows_last_use_and_leaves_unmanaged_entries_alone() {
	let (_temporary, path) = private_root();
	let cache = open(&path, DATA_BYTES * 2);
	for key in [&FIRST, &SECOND] {
		cache
			.with_lock(|directory| {
				super::entry::store(directory, key, BLOCK, (DATA_BYTES * 2) as u64)
			})
			.unwrap();
	}
	let first = entry(&path, &FIRST);
	let second = entry(&path, &SECOND);
	set_age(&first, 1);
	set_age(&second, 2);
	assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), Some(DATA_BYTES));
	fs::write(path.join(UNKNOWN), UNKNOWN_BYTES).unwrap();
	cache.store(&THIRD, BLOCK);
	assert!(first.exists());
	assert!(!second.exists());
	assert_eq!(fs::read(entry(&path, &THIRD)).unwrap(), BLOCK);
	assert_eq!(fs::read(path.join(UNKNOWN)).unwrap(), UNKNOWN_BYTES);
}

#[test]
fn rejects_symlinks_and_public_roots_without_adopting_them() {
	let (_temporary, path) = private_root();
	let directory = path.join("actual");
	let linked = path.join("link");
	fs::create_dir(&directory).unwrap();
	fs::set_permissions(&directory, Permissions::from_mode(PUBLIC_DIRECTORY_MODE)).unwrap();
	assert!(matches!(
		Cache::open(&directory, 1.try_into().unwrap()),
		Err(Error::Privacy)
	));
	fs::set_permissions(&directory, Permissions::from_mode(OWNER_DIRECTORY_MODE)).unwrap();
	symlink(&directory, &linked).unwrap();
	assert!(Cache::open(&linked, 1.try_into().unwrap()).is_err());
	let nested = directory.join("nested");
	fs::create_dir(&nested).unwrap();
	fs::set_permissions(&nested, Permissions::from_mode(OWNER_DIRECTORY_MODE)).unwrap();
	assert!(Cache::open(&linked.join("nested"), 1.try_into().unwrap()).is_err());
	assert!(matches!(
		Cache::open(&nested.join(".."), 1.try_into().unwrap()),
		Err(Error::Path)
	));
}

#[test]
fn opened_root_is_stable_and_links_inside_it_cannot_redirect_io() {
	let (_temporary, path) = private_root();
	let directory = path.join("cache");
	let renamed = path.join("renamed");
	fs::create_dir(&directory).unwrap();
	fs::set_permissions(&directory, Permissions::from_mode(OWNER_DIRECTORY_MODE)).unwrap();
	let cache = open(&directory, DATA_BYTES * 2);
	fs::rename(&directory, &renamed).unwrap();
	fs::create_dir(&directory).unwrap();
	let outside = path.join(UNKNOWN);
	fs::write(&outside, UNKNOWN_BYTES).unwrap();
	symlink(&outside, entry(&renamed, &FIRST)).unwrap();
	assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), None);
	cache.store(&FIRST, BLOCK);
	cache.store(&SECOND, BLOCK);
	assert_eq!(fs::read(&outside).unwrap(), UNKNOWN_BYTES);
	assert!(
		fs::symlink_metadata(entry(&renamed, &FIRST))
			.unwrap()
			.is_symlink()
	);
	assert_eq!(fs::read(entry(&renamed, &SECOND)).unwrap(), BLOCK);
	assert!(!entry(&directory, &SECOND).exists());
}

#[test]
fn does_not_retire_or_replace_cache_names_linked_to_other_files() {
	let (_temporary, path) = private_root();
	let outside_block = path.join(UNKNOWN);
	fs::write(&outside_block, BLOCK).unwrap();
	fs::set_permissions(&outside_block, Permissions::from_mode(OWNER_FILE_MODE)).unwrap();
	let linked = entry(&path, &FIRST);
	fs::hard_link(&outside_block, &linked).unwrap();
	let nlink = fs::metadata(&outside_block).unwrap().nlink();
	let cache = open(&path, DATA_BYTES);
	assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), None);
	cache.store(&FIRST, BLOCK);
	cache.store(&SECOND, BLOCK);
	assert_eq!(fs::metadata(&outside_block).unwrap().nlink(), nlink);
	assert_eq!(fs::read(linked).unwrap(), BLOCK);
	assert!(!entry(&path, &SECOND).exists());
}

#[test]
fn exclusive_locks_on_distinct_handles_serialize_cooperating_instances() {
	let (_temporary, path) = private_root();
	let first = open(&path, DATA_BYTES);
	let second = open(&path, DATA_BYTES);
	let inherited = first
		.with_lock(|directory| {
			assert_eq!(second.load(&FIRST, &mut [0; DATA_BYTES]), None);
			second.store(&FIRST, BLOCK);
			assert!(!entry(&path, &FIRST).exists());
			Ok(rustix::io::fcntl_dupfd_cloexec(directory, 0)?)
		})
		.unwrap();
	second.store(&FIRST, BLOCK);
	assert_eq!(first.load(&FIRST, &mut [0; DATA_BYTES]), Some(DATA_BYTES));
	drop(inherited);
}

#[cfg(target_os = "macos")]
#[test]
fn rejects_darwin_extended_acl_on_owner_only_directory_and_file() {
	let (_temporary, path) = private_root();
	let cache = open(&path, DATA_BYTES);
	cache.store(&FIRST, BLOCK);
	let file = entry(&path, &FIRST);
	for (target, grant) in [
		(file.as_path(), "everyone allow read"),
		(path.as_path(), "everyone allow list,search,add_file"),
	] {
		let status = std::process::Command::new("/bin/chmod")
			.args(["+a", grant])
			.arg(target)
			.status()
			.unwrap();
		assert!(status.success());
		assert_eq!(cache.load(&FIRST, &mut [0; DATA_BYTES]), None);
	}
	assert!(matches!(
		Cache::open(&path, 1.try_into().unwrap()),
		Err(Error::Privacy)
	));
}
