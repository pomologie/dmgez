use std::fs;

use serde_json::json;

use super::{
	DIFFERENT, EQUAL, EXACT_PATHS, NATIVE_PATHS, PATHS_OPTION,
	hfsplus::{HFSPLUS, build},
	report, semantic_command, semantic_diff,
};

const PAYLOAD_SOURCE: &str = "payload.bin";
const ORIGINAL_DATA: &[u8] = b"old payload";
const REVISED_DATA: &[u8] = b"new payload";
const LEFT_DIRECTORY: &str = "Nested";
const RIGHT_DIRECTORY: &str = "nEsTeD";
const LEFT_DESTINATION: &str = "Écho.txt";
const LEFT_STORED_NAME: &str = "E\u{0301}cho.txt";
const RIGHT_DESTINATION: &str = "e\u{0301}C\u{200d}Ho.txt";
const ZERO_WIDTH_JOINER: char = '\u{200d}';
const SPELLING_LINE: &str = "equivalent_path_spelling\t";

#[test]
fn hfsplus_native_pairs_nested_paths_and_reports_both_stored_spellings() {
	let root = tempfile::tempdir().unwrap();
	let source = root.path().join(PAYLOAD_SOURCE);
	let left_entries = file_entry(LEFT_DIRECTORY, LEFT_DESTINATION);
	let right_entries = file_entry(RIGHT_DIRECTORY, RIGHT_DESTINATION);
	fs::write(&source, ORIGINAL_DATA).unwrap();
	let left = build(root.path(), "left", &left_entries);
	let right = build(root.path(), "right", &right_entries);
	fs::write(&source, REVISED_DATA).unwrap();
	let changed = build(root.path(), "changed", &right_entries);

	let exact_mode = [PATHS_OPTION, EXACT_PATHS];
	let baseline = report(&semantic_diff(&left, &right, HFSPLUS, &[]), DIFFERENT);
	let exact = report(
		&semantic_diff(&left, &right, HFSPLUS, &exact_mode),
		DIFFERENT,
	);
	assert_eq!(exact, baseline);
	assert_eq!(exact["policy"]["paths"], "exact_stored_text");
	assert!(exact["policy"].get("native_name_profile").is_none());
	assert!(exact.get("equivalent_path_spellings").is_none());
	assert_eq!(
		exact["changes"],
		json!([
			{"path": [LEFT_DIRECTORY], "kind": "removed", "fields": []},
			{"path": [LEFT_DIRECTORY, LEFT_STORED_NAME], "kind": "removed", "fields": []},
			{"path": [RIGHT_DIRECTORY], "kind": "added", "fields": []},
			{"path": [RIGHT_DIRECTORY, RIGHT_DESTINATION], "kind": "added", "fields": []},
		]),
		"{exact:#}"
	);

	let native_mode = [PATHS_OPTION, NATIVE_PATHS];
	let equal = report(&semantic_diff(&left, &right, HFSPLUS, &native_mode), EQUAL);
	assert_eq!(equal["policy"]["paths"], NATIVE_PATHS);
	assert_eq!(equal["policy"]["native_name_profile"], "hfs_unicode_folded");
	assert_eq!(equal["changes"], json!([]));
	assert_eq!(
		equal["equivalent_path_spellings"],
		json!([
			{"left": [LEFT_DIRECTORY], "right": [RIGHT_DIRECTORY]},
			{
				"left": [LEFT_DIRECTORY, LEFT_STORED_NAME],
				"right": [RIGHT_DIRECTORY, RIGHT_DESTINATION],
			},
		]),
		"{equal:#}"
	);

	let text = semantic_command(&left, &right, HFSPLUS)
		.args(native_mode)
		.output()
		.unwrap();
	assert_eq!(text.status.code(), Some(EQUAL), "{text:?}");
	let text = String::from_utf8(text.stdout).unwrap();
	assert!(!text.contains(ZERO_WIDTH_JOINER), "{text}");
	let spellings: Vec<_> = text
		.lines()
		.filter_map(|line| line.strip_prefix(SPELLING_LINE))
		.map(|pair| {
			let (left, right) = pair.split_once('\t').unwrap();
			json!({
				"left": serde_json::from_str::<Vec<String>>(left).unwrap(),
				"right": serde_json::from_str::<Vec<String>>(right).unwrap(),
			})
		})
		.collect();
	assert_eq!(json!(spellings), equal["equivalent_path_spellings"]);

	let different = report(
		&semantic_diff(&left, &changed, HFSPLUS, &native_mode),
		DIFFERENT,
	);
	assert_eq!(
		different["changes"],
		json!([{
			"path": [LEFT_DIRECTORY, LEFT_STORED_NAME],
			"right_path": [RIGHT_DIRECTORY, RIGHT_DESTINATION],
			"kind": "changed",
			"fields": ["data_fork"],
		}]),
		"{different:#}"
	);
	assert_eq!(
		different["equivalent_path_spellings"],
		json!([{"left": [LEFT_DIRECTORY], "right": [RIGHT_DIRECTORY]}]),
		"{different:#}"
	);
}

fn file_entry(directory: &str, name: &str) -> String {
	format!(
		r#"
[[contents]]
source = "{PAYLOAD_SOURCE}"
destination = "/{directory}/{name}"
"#
	)
}
