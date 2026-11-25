//! D-READ-05-T01 selected fork publication from the native APFS fixture.

use std::{fs, num::NonZeroUsize};

use apfs_reader::TreeLimits;
use dmgez_operations::extract::{
	NameLimits, NameProfile,
	host::{ForkWriteError, ForkWriteLimits, HostError, StableDestination},
	preflight_names,
};
use dmgez_vfs::volume::{ForkKind, LookupName};

use super::{
	DATA_PATTERN, DATA_REPETITIONS, HARD_LINKS, NATIVE_ALIAS, RESOURCE_PATTERN,
	RESOURCE_REPETITIONS,
	fixture::{NativeBlocks, disk, open_container},
	open_view,
};

const NATIVE_DIRECTORY: &str = "Docs";
const DATA_DIRECTORY: &str = "data";
const RESOURCE_DIRECTORY: &str = "resource";
const LIMITS: ForkWriteLimits = ForkWriteLimits {
	max_fork_bytes: 8192,
	chunk_bytes: NonZeroUsize::new(233).unwrap(),
};

#[test]
fn native_apfs_forks_publish_exact_bytes_and_refuse_existing_targets() {
	let blocks = NativeBlocks::from_packed(HARD_LINKS);
	let encoded = blocks.wrap_udif();
	let mut disk = disk(&encoded);
	let container = open_container(&mut disk, &blocks);
	let mut view = open_view(&container, TreeLimits::default());
	let root = view.root().unwrap();
	let folder = view
		.lookup(&root, LookupName::Utf8(NATIVE_DIRECTORY))
		.unwrap()
		.unwrap();
	let names = preflight_names(
		&mut view,
		&folder.node,
		NameProfile::PortableAscii,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	let entry = names
		.entries()
		.iter()
		.find(|entry| entry.parent.is_none() && entry.name.as_str() == NATIVE_ALIAS)
		.unwrap();

	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().canonicalize().unwrap();
	let selections = [
		(
			DATA_DIRECTORY,
			ForkKind::Data,
			ForkKind::Resource,
			DATA_PATTERN.repeat(DATA_REPETITIONS),
		),
		(
			RESOURCE_DIRECTORY,
			ForkKind::Resource,
			ForkKind::Data,
			RESOURCE_PATTERN.repeat(RESOURCE_REPETITIONS),
		),
	];
	for (name, kind, other_kind, expected) in selections {
		let directory = path.join(name);
		fs::create_dir(&directory).unwrap();
		let stable = StableDestination::open(&directory).unwrap();
		let destination = stable.directory();
		let count = view
			.with_fork(&entry.source, kind, |fork| {
				destination.publish_fork(&entry.name, fork, LIMITS, || false)
			})
			.unwrap()
			.unwrap()
			.unwrap();
		assert_eq!(count, expected.len().try_into().unwrap());

		let output = directory.join(entry.name.as_str());
		assert_eq!(fs::read(&output).unwrap(), expected);
		let repeat = view
			.with_fork(&entry.source, other_kind, |fork| {
				destination.publish_fork(&entry.name, fork, LIMITS, || false)
			})
			.unwrap();
		assert!(matches!(
			repeat,
			Some(Err(ForkWriteError::Host(HostError::Conflict)))
		));
		assert_eq!(fs::read(output).unwrap(), expected);
		assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
	}
}
