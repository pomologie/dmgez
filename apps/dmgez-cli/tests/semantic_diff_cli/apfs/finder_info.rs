use std::path::Path;

use serde_json::json;

use super::{
	APFS, BOTH_RAW, DIFFERENT, EQUAL, RAW_WITH_XATTRS, edit_object, materialize, replace_field,
	report, semantic_command, semantic_diff,
};
use crate::{FINDER_INFO_BYTES, FINDER_INFO_FIELD, FINDER_INFO_OPTION};

const LEFT_TYPE_AND_CREATOR: &[u8; 8] = b"TEXTLEFT";
const RIGHT_TYPE_AND_CREATOR: &[u8; 8] = b"TEXTRGHT";

#[test]
fn finder_info_reports_a_shared_apfs_inode_on_each_alias_without_serializing_values() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("left.raw");
	let right = root.path().join("right.raw");
	shared_finder_info_image(&left, LEFT_TYPE_AND_CREATOR);
	shared_finder_info_image(&right, RIGHT_TYPE_AND_CREATOR);

	let baseline = report(&semantic_diff(&left, &right, APFS, BOTH_RAW), EQUAL);
	assert_eq!(baseline["changes"], json!([]));
	assert!(baseline["policy"].get(FINDER_INFO_FIELD).is_none());

	let options = [BOTH_RAW, &[FINDER_INFO_OPTION]].concat();
	let changed = report(&semantic_diff(&left, &right, APFS, &options), DIFFERENT);
	assert_eq!(
		changed["policy"][FINDER_INFO_FIELD]["record_bytes"],
		FINDER_INFO_BYTES
	);
	assert_eq!(
		changed["changes"],
		json!([
			{"path": ["Docs", "alias.bin"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
			{"path": ["More", "Écho.bin"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
			{"path": ["primary.bin"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
		]),
		"{changed:#}"
	);
	let with_xattrs = [RAW_WITH_XATTRS, &[FINDER_INFO_OPTION]].concat();
	let both = report(&semantic_diff(&left, &right, APFS, &with_xattrs), DIFFERENT);
	assert_eq!(both["changes"], changed["changes"]);
	assert!(both["policy"]["xattrs"].is_object());

	let text = semantic_command(&left, &right, APFS)
		.args(options)
		.output()
		.unwrap();
	assert_eq!(text.status.code(), Some(DIFFERENT), "{text:?}");
	let stdout = String::from_utf8(text.stdout).unwrap();
	let expected_line = format!("changed\t[\"primary.bin\"]\t{FINDER_INFO_FIELD}");
	assert!(stdout.lines().any(|line| line == expected_line), "{stdout}");
	for value in [LEFT_TYPE_AND_CREATOR, RIGHT_TYPE_AND_CREATOR] {
		assert!(!stdout.contains(std::str::from_utf8(value).unwrap()));
	}
}

fn shared_finder_info_image(image: &Path, type_and_creator: &[u8; 8]) {
	const FSROOT_BLOCK: u64 = 206;
	const INODE_FLAGS_AT: usize = 3_339;
	const NATIVE_INODE_FLAGS: u64 = 1 << 14;
	const INODE_HAS_FINDER_INFO: u64 = 1 << 8;
	const PROVENANCE_NAME_AT: usize = 499;
	const PROVENANCE: [u8; 20] = *b"com.apple.provenance";
	const FINDER_INFO: [u8; 20] = *b"com.apple.FinderInfo";
	const RESOURCE_SLOT_AT: usize = 152;
	const PROVENANCE_SLOT_AT: usize = 160;
	const RESOURCE_SLOT: [u8; 8] = [232, 0, 33, 0, 38, 2, 52, 0];
	const PROVENANCE_SLOT: [u8; 8] = [177, 0, 31, 0, 53, 2, 15, 0];
	const SLOT_VALUE_OFFSET: usize = 2 * size_of::<u16>();
	const FREE_BYTES_AT: usize = 46;
	const NATIVE_FREE_BYTES: u16 = 2_252;
	const VALUE_REGION_START: usize = 3_019;
	const VALUE_REGION_END: usize = 4_056;
	const INLINE_FLAG: u16 = 1 << 1;
	const INLINE_HEADER_BYTES: usize = 2 * size_of::<u16>();
	const VALUE_BYTES: usize = INLINE_HEADER_BYTES + FINDER_INFO_BYTES;
	const VALUE_AT: usize = VALUE_REGION_START - VALUE_BYTES;

	materialize(image, super::HARD_LINK_FIXTURE, &[]);
	edit_object(image, FSROOT_BLOCK, |block| {
		replace_field(
			block,
			INODE_FLAGS_AT,
			NATIVE_INODE_FLAGS.to_le_bytes(),
			(NATIVE_INODE_FLAGS | INODE_HAS_FINDER_INFO).to_le_bytes(),
		);
		replace_field(block, PROVENANCE_NAME_AT, PROVENANCE, FINDER_INFO);

		// Preserve xattr name order by swapping the two TOC entries. Grow
		// the value region at its low end into the native free space.
		let mut finder_slot = PROVENANCE_SLOT;
		let offset = u16::try_from(VALUE_REGION_END - VALUE_AT)
			.unwrap()
			.to_le_bytes();
		let length = u16::try_from(VALUE_BYTES).unwrap().to_le_bytes();
		finder_slot[SLOT_VALUE_OFFSET..][..size_of::<u16>()].copy_from_slice(&offset);
		finder_slot[SLOT_VALUE_OFFSET + size_of::<u16>()..].copy_from_slice(&length);
		replace_field(block, RESOURCE_SLOT_AT, RESOURCE_SLOT, finder_slot);
		replace_field(block, PROVENANCE_SLOT_AT, PROVENANCE_SLOT, RESOURCE_SLOT);
		replace_field(
			block,
			FREE_BYTES_AT,
			NATIVE_FREE_BYTES.to_le_bytes(),
			(NATIVE_FREE_BYTES - u16::from_le_bytes(length)).to_le_bytes(),
		);
		let mut value = [0; VALUE_BYTES];
		value[..size_of::<u16>()].copy_from_slice(&INLINE_FLAG.to_le_bytes());
		value[size_of::<u16>()..INLINE_HEADER_BYTES]
			.copy_from_slice(&u16::try_from(FINDER_INFO_BYTES).unwrap().to_le_bytes());
		value[INLINE_HEADER_BYTES..][..type_and_creator.len()].copy_from_slice(type_and_creator);
		replace_field(block, VALUE_AT, [0; VALUE_BYTES], value);
	});
}
