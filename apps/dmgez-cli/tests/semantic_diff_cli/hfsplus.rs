use std::fs;

#[cfg(feature = "project-build")]
use std::{
	path::{Path, PathBuf},
	process::Command,
};

use serde_json::json;

#[cfg(feature = "project-build")]
use super::BINARY;
use super::{
	DIFFERENT, EQUAL,
	native_hfsplus::{hfsplus_fixture, materialize_sparse},
	report, semantic_diff,
};

const HFSPLUS: &str = "hfsplus";
const BOTH_RAW: &[&str] = &["--left-raw", "--right-raw"];
const HFSX_FIXTURE: &str = "native-hfsx.bin";
const HFSX_VOLUME_BYTES: usize = 24 * (1 << 20);
const UPPERCASE_FILE_DATA: &[u8] = b"capital\0\x01\xff\n";
const UPPERCASE_FILE_OFFSET: usize = 4_530_176;

#[test]
fn native_hfsx_distinguishes_the_two_case_sensitive_file_names() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("same-case-tree.raw");
	let right = root.path().join("modified-case-tree.raw");
	let mut bytes = materialize_sparse(&hfsplus_fixture(HFSX_FIXTURE), HFSX_VOLUME_BYTES);
	fs::write(&left, &bytes).unwrap();

	let output = semantic_diff(&left, &left, HFSPLUS, BOTH_RAW);
	let equal = report(&output, EQUAL);
	assert_eq!(equal["outcome"], "equal", "{equal:#}");
	assert_eq!(equal["left"]["hfs_signature"], "hfsx");
	assert_eq!(equal["right"]["hfs_signature"], "hfsx");

	let fork = &mut bytes[UPPERCASE_FILE_OFFSET..][..UPPERCASE_FILE_DATA.len()];
	assert_eq!(fork, UPPERCASE_FILE_DATA);
	fork[0] ^= 1;
	fs::write(&right, bytes).unwrap();
	let output = semantic_diff(&left, &right, HFSPLUS, BOTH_RAW);
	let changed = report(&output, DIFFERENT);
	assert_eq!(
		changed["changes"],
		json!([{"path": ["Cases", "Case.txt"], "kind": "changed", "fields": ["data_fork"]}]),
		"{changed:#}"
	);
}

#[cfg(feature = "project-build")]
#[test]
fn preserves_stored_spelling_and_compares_inert_symbolic_link_targets() {
	const PAYLOAD_FILE: &str = "shared.bin";
	const PAYLOAD: &[u8] = b"unchanged regular file\x00\xff\n";
	const LEFT_ENTRIES: &str = r#"
[[contents]]
source = "shared.bin"
destination = "/Case.txt"
[[contents]]
source = "shared.bin"
destination = "/Removed.txt"
[[contents]]
type = "symlink"
destination = "/Link"
target = "/missing-left"
"#;
	const RIGHT_ENTRIES: &str = r#"
[[contents]]
source = "shared.bin"
destination = "/case.txt"
[[contents]]
source = "shared.bin"
destination = "/Added.txt"
[[contents]]
type = "symlink"
destination = "/Link"
target = "/missing-right"
"#;

	let root = tempfile::tempdir().unwrap();
	fs::write(root.path().join(PAYLOAD_FILE), PAYLOAD).unwrap();
	let left = build(root.path(), "left", LEFT_ENTRIES);
	let right = build(root.path(), "right", RIGHT_ENTRIES);
	let output = semantic_diff(&left, &right, HFSPLUS, &[]);
	let report = report(&output, DIFFERENT);
	assert_eq!(report["left"]["hfs_signature"], HFSPLUS);
	assert_eq!(report["right"]["hfs_signature"], HFSPLUS);
	let changes = report["changes"].as_array().unwrap();
	for (path, kind, fields) in [
		("Case.txt", "removed", json!([])),
		("case.txt", "added", json!([])),
		("Removed.txt", "removed", json!([])),
		("Added.txt", "added", json!([])),
		("Link", "changed", json!(["symlink_target"])),
	] {
		assert!(
			changes.iter().any(|change| change["path"] == json!([path])
				&& change["kind"] == kind
				&& change["fields"] == fields),
			"missing {kind} {path}: {report:#}",
		);
	}
}

#[cfg(feature = "project-build")]
#[test]
fn compares_visible_hard_link_groups_with_identical_payloads() {
	const PAYLOAD: &[u8] = b"same bytes under each visible path\n";
	const LINKED_ENTRIES: &str = r#"
[[contents]]
source = "original.bin"
destination = "/Original.bin"
[[contents]]
source = "alias.bin"
destination = "/Alias.bin"
"#;
	const SEPARATE_ENTRIES: &str = r#"
[[contents]]
source = "original.bin"
destination = "/Original.bin"
[[contents]]
source = "separate.bin"
destination = "/Alias.bin"
"#;
	let root = tempfile::tempdir().unwrap();
	let original = root.path().join("original.bin");
	fs::write(&original, PAYLOAD).unwrap();
	fs::hard_link(&original, root.path().join("alias.bin")).unwrap();
	fs::write(root.path().join("separate.bin"), PAYLOAD).unwrap();
	let left = build(root.path(), "linked", LINKED_ENTRIES);
	let right = build(root.path(), "separate", SEPARATE_ENTRIES);

	let output = semantic_diff(&left, &right, HFSPLUS, &[]);
	let report = report(&output, DIFFERENT);
	let changes = report["changes"].as_array().unwrap();
	for path in ["Alias.bin", "Original.bin"] {
		assert!(
			changes.iter().any(|change| change["path"] == json!([path])
				&& change["kind"] == "changed"
				&& change["fields"] == json!(["hard_link_topology"])),
			"missing topology change at {path}: {report:#}",
		);
	}
}

#[cfg(feature = "project-build")]
fn build(root: &Path, name: &str, entries: &str) -> PathBuf {
	let project = root.join(name).with_extension("toml");
	let image = root.join(name).with_extension("dmg");
	fs::write(
		&project,
		format!(
			r#"
schema_version = 1
[image]
name = "Semantic Diff Witness"
output = "{name}.dmg"
format = "udro"
size = "1MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = false
preserve_hard_links = true
{entries}
"#
		),
	)
	.unwrap();
	let built = Command::new(BINARY)
		.arg("build")
		.arg(project)
		.output()
		.unwrap();
	assert!(built.status.success(), "{built:?}");
	image
}
