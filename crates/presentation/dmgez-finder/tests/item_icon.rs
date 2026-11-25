#![cfg(feature = "volume-icon")]

use dmgez_finder::{
	item_icon::{
		Error, ICON_FILE_NAME, RESOURCE_FORK_OVERHEAD_BYTES, encode_resource_fork,
		folder_icon_file_finder_info, mark_custom_icon,
	},
	volume_icon,
};

const ICNS: &[u8] = include_bytes!("fixtures/apple-iconutil.icns");
const HEADER_BYTES: usize = 16;
const FINDER_INFO_BYTES: usize = 32;
const FLAGS: std::ops::Range<usize> = 8..10;

#[test]
fn emits_one_unnamed_custom_icon_with_the_exact_icns_payload() {
	let limit = RESOURCE_FORK_OVERHEAD_BYTES + ICNS.len();
	let fork = encode_resource_fork(ICNS, limit).unwrap();
	assert_eq!(fork.len(), limit);
	assert_eq!(fork, encode_resource_fork(ICNS, limit).unwrap());

	let data_start = word(&fork, 0) as usize;
	let map_start = word(&fork, 4) as usize;
	let data_len = word(&fork, 8) as usize;
	let map_len = word(&fork, 12) as usize;
	assert_eq!(data_start, 256);
	assert_eq!(data_start + data_len, map_start);
	assert_eq!(map_start + map_len, fork.len());
	assert_eq!(&fork[HEADER_BYTES..data_start], &[0; 256 - HEADER_BYTES]);

	assert_eq!(word(&fork, data_start) as usize, ICNS.len());
	assert_eq!(&fork[data_start + size_of::<u32>()..map_start], ICNS);

	let map = &fork[map_start..];
	assert_eq!(map.len(), 50);
	assert_eq!(map[..HEADER_BYTES], fork[..HEADER_BYTES]);
	assert_eq!(&map[HEADER_BYTES..24], &[0; 8]);
	let types = half(map, 24) as usize;
	let names = half(map, 26) as usize;
	assert_eq!(types, 28);
	assert_eq!(names, map.len());
	assert_eq!(half(map, types), 0);
	assert_eq!(&map[types + 2..types + 6], b"icns");
	assert_eq!(half(map, types + 6), 0);

	let reference = types + half(map, types + 8) as usize;
	assert_eq!(reference, 38);
	assert_eq!(half(map, reference).cast_signed(), -16_455);
	assert_eq!(half(map, reference + 2).cast_signed(), -1);
	assert_eq!(word(map, reference + 4), 0);
	assert_eq!(word(map, reference + 8), 0);
}

#[test]
fn refuses_an_invalid_icon_and_a_short_output_budget() {
	assert!(matches!(
		encode_resource_fork(b"bad icon", usize::MAX),
		Err(Error::Icon(volume_icon::Error::FileHeader))
	));
	let limit = RESOURCE_FORK_OVERHEAD_BYTES + ICNS.len();
	assert!(matches!(
		encode_resource_fork(ICNS, limit - 1),
		Err(Error::OutputBudget)
	));
}

#[test]
fn marks_the_item_without_overwriting_any_other_finder_information() {
	let before = [0x81; FINDER_INFO_BYTES];
	let mut expected = before;
	expected[FLAGS].copy_from_slice(&0x8581u16.to_be_bytes());
	let mut actual = before;
	mark_custom_icon(&mut actual);
	assert_eq!(actual, expected);
	mark_custom_icon(&mut actual);
	assert_eq!(actual, expected);

	assert_eq!(ICON_FILE_NAME.as_bytes(), b"Icon\r");
	let mut child = [0; FINDER_INFO_BYTES];
	child[FLAGS].copy_from_slice(&0x4000u16.to_be_bytes());
	assert_eq!(folder_icon_file_finder_info(), child);
}

fn word(bytes: &[u8], offset: usize) -> u32 {
	u32::from_be_bytes(bytes[offset..][..size_of::<u32>()].try_into().unwrap())
}

fn half(bytes: &[u8], offset: usize) -> u16 {
	u16::from_be_bytes(bytes[offset..][..size_of::<u16>()].try_into().unwrap())
}
