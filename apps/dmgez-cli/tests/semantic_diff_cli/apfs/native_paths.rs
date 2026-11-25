use std::path::Path;

use serde_json::json;

use super::super::{NATIVE_PATHS, PATHS_OPTION};
use super::{
	APFS, BOTH_RAW, DIFFERENT, EQUAL, HARD_LINK_FIXTURE, SPARSE_FIXTURE, assert_error, edit_object,
	materialize, replace_field, report, semantic_diff,
};

const FOLDED_PROFILE: &str = "apfs_unicode9_folded";
const FSROOT_BLOCK: u64 = 212;
const DIRECTORY_NAME_AT: usize = 317;
const INODE_NAME_AT: usize = 3543;
const PACKED_NAME_INFO_AT: usize = DIRECTORY_NAME_AT - size_of::<u32>();
const ORIGINAL_NAME: &[u8; 10] = b"sparse.bin";
const ORIGINAL_NAME_INFO: u32 = 0x6317_080b;
const FIRST_SPELLING: &str = "xÉe\u{301}.bin";
const SECOND_SPELLING: &str = "XE\u{301}é.BIN";
const FOLDED_NAME_INFO: u32 = 0x11e8_f40b;
const NEWER_NAME: &str = "\u{1fae0}xy.bin";
const NEWER_NAME_INFO: u32 = 0x288a_c40b;

#[test]
fn native_paths_report_the_folded_profile_on_an_unchanged_apfs_catalog() {
	let root = tempfile::tempdir().unwrap();
	let image = root.path().join("native-catalog.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);

	let exact = report(&semantic_diff(&image, &image, APFS, BOTH_RAW), EQUAL);
	assert_eq!(exact["policy"]["paths"], "exact_stored_text");
	assert!(exact["policy"].get("native_name_profile").is_none());

	let options = [BOTH_RAW, &[PATHS_OPTION, NATIVE_PATHS]].concat();
	let native = report(&semantic_diff(&image, &image, APFS, &options), EQUAL);
	assert_eq!(native["policy"]["paths"], NATIVE_PATHS);
	assert_eq!(native["policy"]["native_name_profile"], FOLDED_PROFILE);
	assert_eq!(native["changes"], json!([]));
	assert!(native.get("equivalent_path_spellings").is_none());
}

#[test]
fn native_paths_pair_mixed_case_and_composition_on_apfs() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("first-spelling.raw");
	let right = root.path().join("second-spelling.raw");
	sparse_image_with_spelling(&left, FIRST_SPELLING, FOLDED_NAME_INFO);
	sparse_image_with_spelling(&right, SECOND_SPELLING, FOLDED_NAME_INFO);

	let exact = report(&semantic_diff(&left, &right, APFS, BOTH_RAW), DIFFERENT);
	assert_eq!(
		exact["changes"],
		json!([
			{"kind": "added", "path": [SECOND_SPELLING], "fields": []},
			{"kind": "removed", "path": [FIRST_SPELLING], "fields": []},
		]),
	);
	let options = [BOTH_RAW, &[PATHS_OPTION, NATIVE_PATHS]].concat();
	let native = report(&semantic_diff(&left, &right, APFS, &options), EQUAL);
	assert_eq!(native["policy"]["native_name_profile"], FOLDED_PROFILE);
	assert_eq!(native["changes"], json!([]));
	assert_eq!(
		native["equivalent_path_spellings"],
		json!([{"left": [FIRST_SPELLING], "right": [SECOND_SPELLING]}]),
	);
}

#[test]
fn native_paths_refuse_an_apfs_scalar_outside_the_qualified_profile() {
	let root = tempfile::tempdir().unwrap();
	let image = root.path().join("newer-scalar.raw");
	sparse_image_with_spelling(&image, NEWER_NAME, NEWER_NAME_INFO);

	let exact = report(&semantic_diff(&image, &image, APFS, BOTH_RAW), EQUAL);
	assert_eq!(exact["changes"], json!([]));
	let options = [BOTH_RAW, &[PATHS_OPTION, NATIVE_PATHS]].concat();
	let native = semantic_diff(&image, &image, APFS, &options);
	assert_error(
		&native,
		"could not obtain a native name key for the left path",
	);
	assert_error(&native, "U+1FAE0");
}

fn sparse_image_with_spelling(image: &Path, name: &str, packed_name_info: u32) {
	materialize(image, SPARSE_FIXTURE, &[]);
	let rewritten = name.as_bytes().try_into().unwrap();
	edit_object(image, FSROOT_BLOCK, |block| {
		// These equal-length synthetic paths keep both node slots and the
		// inode's name xfield intact. Their CRC32C name hashes still sort
		// before the existing all-hole.bin record in this captured root.
		replace_field(
			block,
			PACKED_NAME_INFO_AT,
			ORIGINAL_NAME_INFO.to_le_bytes(),
			packed_name_info.to_le_bytes(),
		);
		replace_field(block, DIRECTORY_NAME_AT, *ORIGINAL_NAME, rewritten);
		replace_field(block, INODE_NAME_AT, *ORIGINAL_NAME, rewritten);
	});
}
