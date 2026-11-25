//! D-READ-05-T01 selected fork publication from the native HFS+ fixture.

use std::{fs, num::NonZeroUsize};

use dmgez_operations::extract::{
	NameLimits, NameProfile,
	host::{ForkWriteError, ForkWriteLimits, HostError, StableDestination},
	preflight_names,
};
use dmgez_vfs::volume::ForkKind;

use super::{DATA, HARD_LINKS, RESOURCE, materialize, with_image};

const DATA_OUTPUT: &str = "one";
const RESOURCE_OUTPUT: &str = "two";
const LIMITS: ForkWriteLimits = ForkWriteLimits {
	max_fork_bytes: 4096,
	chunk_bytes: NonZeroUsize::new(7).unwrap(),
};

#[test]
fn native_hfs_forks_publish_exact_bytes_and_refuse_existing_targets() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().canonicalize().unwrap();
	let stable = StableDestination::open(&path).unwrap();
	let destination = stable.directory();

	with_image(&materialize(HARD_LINKS), |view| {
		let root = view.root().unwrap();
		let names = preflight_names(
			view,
			&root,
			NameProfile::PortableAscii,
			NameLimits::default(),
			|| false,
		)
		.unwrap();
		let selections = [
			(DATA_OUTPUT, ForkKind::Data, ForkKind::Resource, DATA),
			(
				RESOURCE_OUTPUT,
				ForkKind::Resource,
				ForkKind::Data,
				RESOURCE,
			),
		];
		for (name, kind, other_kind, expected) in selections {
			let entry = names
				.entries()
				.iter()
				.find(|entry| entry.parent.is_none() && entry.name.as_str() == name)
				.unwrap();
			let count = view
				.with_fork(&entry.source, kind, |fork| {
					destination.publish_fork(&entry.name, fork, LIMITS, || false)
				})
				.unwrap()
				.unwrap()
				.unwrap();
			assert_eq!(count, expected.len().try_into().unwrap());

			let output = path.join(name);
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
		}
	});
	assert_eq!(fs::read_dir(path).unwrap().count(), 2);
}
