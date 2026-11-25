use dmgez_hfsplus_read::browse::VolumeSignature;
use dmgez_vfs::{VolumeId, volume::RawName};

use super::*;

fn page(name: &str, offset: u64) -> Page {
	let root = Location::root(VolumeId::WholeDisk);
	let name = RawName::Utf16(name.encode_utf16().collect());
	Page {
		location: root.child(&name).unwrap(),
		filesystem: VolumeSignature::HfsPlus,
		entries: Vec::new(),
		offset,
		next: None,
		truncated: false,
	}
}

#[test]
fn successful_arrivals_restore_page_offsets_and_clear_forward_on_a_new_branch() {
	let mut history = History::default();
	let first = page("first", 0);
	let second_page = page("first", 64);
	let other = page("other", 0);
	let third = page("third", 0);

	assert!(history.arrived(None, &first, None));
	assert!(history.arrived(Some(&first), &second_page, None));
	assert!(!history.available(Direction::Back));
	assert!(history.arrived(Some(&second_page), &other, None));
	assert!(matches!(
		history.request(Direction::Back),
		Some(Request::Page { location, offset: 64 }) if location == first.location
	));
	assert!(!history.arrived(Some(&other), &first, Some(Direction::Back)));
	assert!(history.available(Direction::Back));
	assert!(!history.available(Direction::Forward));
	assert!(history.arrived(Some(&other), &second_page, Some(Direction::Back)));
	assert!(history.available(Direction::Forward));
	assert!(history.arrived(Some(&second_page), &other, Some(Direction::Forward)));
	assert!(history.arrived(Some(&other), &second_page, Some(Direction::Back)));
	assert!(history.arrived(Some(&second_page), &third, None));
	assert!(!history.available(Direction::Forward));
}

#[test]
fn a_long_navigation_sequence_keeps_only_the_latest_folders() {
	let mut history = History::default();
	let mut previous = page("0", 0);
	for index in 1..=MAX_HISTORY + 2 {
		let next = page(&index.to_string(), 0);
		assert!(history.arrived(Some(&previous), &next, None));
		previous = next;
	}
	assert_eq!(history.back.len(), MAX_HISTORY);
	assert_eq!(
		history.back.front().unwrap().location,
		page("2", 0).location
	);
}
