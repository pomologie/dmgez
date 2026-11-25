use std::{
	fs,
	path::{Path, PathBuf},
	process::Command,
};

use serde_json::json;

use super::{
	BINARY, DIFFERENT, EQUAL, FINDER_INFO_FIELD, FINDER_INFO_OPTION, FINDER_LAYOUT_OPTION,
	XATTR_OPTION, assert_error,
	hfsplus::{BOTH_RAW, HFSPLUS, build},
	report, semantic_command, semantic_diff,
};

const STORE_NAME: &str = ".DS_Store";
const ITEM: &str = "Example";
const INITIAL_X: u32 = 150;
const MOVED_X: u32 = 190;
const ICON_LOCATION: &[u8; 4] = b"Iloc";
const BLOB_TYPE: &[u8; 4] = b"blob";
const UNKNOWN_TYPE: &[u8; 4] = b"unkn";
const MAGIC_OFFSET: usize = size_of::<u32>();
// The buddy header's alignment, magic, allocator offset, length and repeated offset precede padding.
const HEADER_PADDING_OFFSET: usize = 5 * size_of::<u32>();
const FILL_BYTE: u8 = 0xa5;

#[test]
fn reports_the_decoded_icon_change_alongside_xattr_and_finder_info_policies() {
	let root = tempfile::tempdir().unwrap();
	let left = build_layout(root.path(), "left", INITIAL_X);
	let right = build_layout(root.path(), "right", MOVED_X);
	let baseline = report(&semantic_diff(&left, &right, HFSPLUS, &[]), DIFFERENT);
	assert_eq!(
		baseline["changes"],
		json!([{"path": [STORE_NAME], "kind": "changed", "fields": ["data_fork"]}]),
		"{baseline:#}"
	);
	assert!(baseline["policy"].get("finder_layout").is_none());

	let decoded = report(
		&semantic_diff(&left, &right, HFSPLUS, &[FINDER_LAYOUT_OPTION]),
		DIFFERENT,
	);
	assert_eq!(decoded["policy"]["finder_layout"]["store_name"], STORE_NAME);
	assert_eq!(
		decoded["changes"],
		json!([{
			"path": [STORE_NAME],
			"kind": "changed",
			"fields": ["finder_layout"],
			"finder_layout": [{
				"name": ITEM,
				"field": ICON_LOCATION,
				"kind": "changed",
			}],
		}]),
		"{decoded:#}"
	);

	let combined = report(
		&semantic_diff(
			&left,
			&right,
			HFSPLUS,
			&[XATTR_OPTION, FINDER_LAYOUT_OPTION, FINDER_INFO_OPTION],
		),
		DIFFERENT,
	);
	assert_eq!(combined["changes"], decoded["changes"], "{combined:#}");
	assert_eq!(
		combined["policy"]["finder_layout"],
		decoded["policy"]["finder_layout"]
	);
	assert!(combined["policy"]["xattrs"].is_object(), "{combined:#}");
	assert!(
		combined["policy"][FINDER_INFO_FIELD].is_object(),
		"{combined:#}"
	);

	let text = semantic_command(&left, &right, HFSPLUS)
		.arg(FINDER_LAYOUT_OPTION)
		.output()
		.unwrap();
	assert_eq!(text.status.code(), Some(DIFFERENT), "{text:?}");
	let stdout = String::from_utf8(text.stdout).unwrap();
	let records = stdout
		.lines()
		.find_map(|line| line.split_once("\tfinder_layout = "))
		.expect("Finder record changes in text report")
		.1;
	assert_eq!(
		serde_json::from_str::<serde_json::Value>(records).unwrap(),
		decoded["changes"][0]["finder_layout"],
	);
}

#[test]
fn equal_finder_facts_ignore_different_buddy_header_padding() {
	let root = tempfile::tempdir().unwrap();
	let image = build_layout(root.path(), "source", INITIAL_X);
	let store = read_store(&image);
	let left = unwrap_raw(&image);
	let right = root.path().join("different-padding.raw");
	let mut raw = fs::read(&left).unwrap();
	let found = store_in_raw(&mut raw, &store);
	assert_eq!(found[HEADER_PADDING_OFFSET], 0);
	found[HEADER_PADDING_OFFSET] = FILL_BYTE;
	fs::write(&right, raw).unwrap();

	let baseline = report(&semantic_diff(&left, &right, HFSPLUS, BOTH_RAW), DIFFERENT);
	assert_eq!(baseline["changes"][0]["fields"], json!(["data_fork"]));
	let options = [BOTH_RAW, &[FINDER_LAYOUT_OPTION]].concat();
	let equal = report(&semantic_diff(&left, &right, HFSPLUS, &options), EQUAL);
	assert_eq!(equal["outcome"], "equal", "{equal:#}");
	assert_eq!(equal["changes"], json!([]), "{equal:#}");
}

#[test]
fn refuses_malformed_and_unsupported_stores_without_a_partial_report() {
	let root = tempfile::tempdir().unwrap();
	let image = build_layout(root.path(), "source", INITIAL_X);
	let store = read_store(&image);
	let left = unwrap_raw(&image);
	let mut raw = fs::read(&left).unwrap();
	let right = root.path().join("malformed.raw");
	store_in_raw(&mut raw, &store)[MAGIC_OFFSET] = FILL_BYTE;
	fs::write(&right, raw).unwrap();
	let options = [BOTH_RAW, &[FINDER_LAYOUT_OPTION]].concat();
	let malformed = semantic_diff(&left, &right, HFSPLUS, &options);
	assert_error(&malformed, "could not compare the right Finder layout");
	assert_error(&malformed, STORE_NAME);
	assert_error(&malformed, "invalid Finder store header");

	let mut raw = fs::read(&left).unwrap();
	let found = store_in_raw(&mut raw, &store);
	let tags = [ICON_LOCATION.as_slice(), BLOB_TYPE.as_slice()].concat();
	let offset = found
		.windows(tags.len())
		.position(|bytes| bytes == tags)
		.unwrap()
		+ ICON_LOCATION.len();
	found[offset..][..BLOB_TYPE.len()].copy_from_slice(UNKNOWN_TYPE);
	let right = root.path().join("unsupported.raw");
	fs::write(&right, raw).unwrap();
	let unsupported = semantic_diff(&left, &right, HFSPLUS, &options);
	assert_error(&unsupported, "could not compare the right Finder layout");
	assert_error(&unsupported, "unrecognized Finder value type");
}

#[test]
fn finder_store_reads_count_toward_the_payload_budget() {
	let root = tempfile::tempdir().unwrap();
	let image = build_layout(root.path(), "source", INITIAL_X);
	let store_len = read_store(&image).len();
	let available = store_len.to_string();
	let exact = report(
		&semantic_diff(
			&image,
			&image,
			HFSPLUS,
			&[FINDER_LAYOUT_OPTION, "--max-bytes", &available],
		),
		EQUAL,
	);
	assert_eq!(exact["compared_bytes"], available, "{exact:#}");

	let limited = (store_len - 1).to_string();
	let exhausted = semantic_diff(
		&image,
		&image,
		HFSPLUS,
		&[FINDER_LAYOUT_OPTION, "--max-bytes", &limited],
	);
	assert_error(
		&exhausted,
		&format!("semantic comparison exhausted its {limited} byte payload budget"),
	);
}

fn build_layout(root: &Path, name: &str, x: u32) -> PathBuf {
	let entries = format!(
		r#"
[[contents]]
type = "directory"
destination = "/{ITEM}"
position = [{x}, 175]
[finder]
window_size = [620, 270]
"#
	);
	build(root, name, &entries)
}

fn read_store(image: &Path) -> Vec<u8> {
	let output = Command::new(BINARY)
		.arg("cat")
		.arg(image)
		.arg(format!("/{STORE_NAME}"))
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	assert!(!output.stdout.is_empty());
	output.stdout
}

fn unwrap_raw(image: &Path) -> PathBuf {
	let raw = image.with_extension("raw");
	let output = Command::new(BINARY)
		.arg("unwrap-raw")
		.arg(image)
		.arg("-o")
		.arg(&raw)
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	raw
}

fn store_in_raw<'raw>(raw: &'raw mut [u8], store: &[u8]) -> &'raw mut [u8] {
	let mut matches = raw
		.windows(store.len())
		.enumerate()
		.filter_map(|(offset, bytes)| (bytes == store).then_some(offset));
	let offset = matches
		.next()
		.expect("generated Finder store in raw HFS+ image");
	assert!(matches.next().is_none());
	&mut raw[offset..][..store.len()]
}
