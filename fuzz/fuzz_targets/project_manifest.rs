#![no_main]

use std::collections::BTreeSet;

use dmgez_config::{Capacity, Project};
use dmgez_udif::SECTOR_BYTES;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
	let Ok(text) = str::from_utf8(bytes) else {
		return;
	};
	let Ok(project) = Project::parse(text) else {
		return;
	};
	if let Capacity::Exact(size) | Capacity::ShrinkWithin(size) = project.image.capacity() {
		assert!(size.get() > 0);
		assert!(size.get().is_multiple_of(SECTOR_BYTES));
		assert!(project.image.free_space <= size);
	}

	let mut names = BTreeSet::new();
	for content in &project.contents {
		assert!(!content.destination.is_root());
		assert!(names.insert(content.destination.as_str()));
	}
});
