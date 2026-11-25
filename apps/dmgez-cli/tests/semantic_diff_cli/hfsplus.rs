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
	DIFFERENT, EQUAL, FINDER_INFO_BYTES, FINDER_INFO_FIELD, FINDER_INFO_OPTION, NATIVE_PATHS,
	PATHS_OPTION, XATTR_OPTION, assert_error,
	native_hfsplus::{hfsplus_fixture, materialize_sparse, overlay_sparse},
	report, semantic_diff,
};

pub(super) const HFSPLUS: &str = "hfsplus";
pub(super) const BOTH_RAW: &[&str] = &["--left-raw", "--right-raw"];
const RAW_WITH_XATTRS: &[&str] = &["--left-raw", "--right-raw", XATTR_OPTION];
const RAW_WITH_NATIVE_PATHS: &[&str] = &["--left-raw", "--right-raw", PATHS_OPTION, NATIVE_PATHS];
const HFSX_FIXTURE: &str = "native-hfsx.bin";
const HFSX_VOLUME_BYTES: usize = 24 * (1 << 20);
const HFSX_BINARY: &str = "hfsx_binary";
const UPPERCASE_FILE_DATA: &[u8] = b"capital\0\x01\xff\n";
const UPPERCASE_FILE_OFFSET: usize = 4_530_176;
const CATALOG_FINDER_INFO_OFFSET: usize = 48;
const CATALOG_RECORD_KIND_BYTES: usize = size_of::<u16>();
const CATALOG_FILE: u16 = 2;
const CATALOG_FOLDER: u16 = 1;
const FINDER_TYPE_AND_CREATOR: [u8; 8] = *b"TEXTTEST";

#[test]
fn native_hfsx_distinguishes_the_two_case_sensitive_file_names() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("same-case-tree.raw");
	let right = root.path().join("modified-case-tree.raw");
	let mut bytes = materialize_sparse(&hfsplus_fixture(HFSX_FIXTURE), HFSX_VOLUME_BYTES);
	fs::write(&left, &bytes).unwrap();

	let output = semantic_diff(&left, &left, HFSPLUS, RAW_WITH_XATTRS);
	let equal = report(&output, EQUAL);
	assert_eq!(equal["outcome"], "equal", "{equal:#}");
	assert_eq!(equal["left"]["hfs_signature"], "hfsx");
	assert_eq!(equal["right"]["hfs_signature"], "hfsx");
	assert!(equal["policy"]["xattrs"].is_object());

	// This catalog contains both Case.txt and case.txt under Cases. Binary
	// native pairing must preserve both names without a fold-key conflict.
	let binary = report(
		&semantic_diff(&left, &left, HFSPLUS, RAW_WITH_NATIVE_PATHS),
		EQUAL,
	);
	assert_eq!(binary["policy"]["paths"], NATIVE_PATHS);
	assert_eq!(binary["policy"]["native_name_profile"], HFSX_BINARY);
	assert_eq!(binary["changes"], json!([]));
	assert!(binary.get("equivalent_path_spellings").is_none());

	let fork = &mut bytes[UPPERCASE_FILE_OFFSET..][..UPPERCASE_FILE_DATA.len()];
	assert_eq!(fork, UPPERCASE_FILE_DATA);
	fork[0] ^= 1;
	fs::write(&right, bytes).unwrap();
	let output = semantic_diff(&left, &right, HFSPLUS, RAW_WITH_XATTRS);
	let changed = report(&output, DIFFERENT);
	assert_eq!(
		changed["changes"],
		json!([{"path": ["Cases", "Case.txt"], "kind": "changed", "fields": ["data_fork"]}]),
		"{changed:#}"
	);
	let binary_changed = report(
		&semantic_diff(&left, &right, HFSPLUS, RAW_WITH_NATIVE_PATHS),
		DIFFERENT,
	);
	assert_eq!(binary_changed["changes"], changed["changes"]);
}

#[test]
fn native_path_comparison_checks_hfs_catalog_collations() {
	const STANDARD_CATALOG: &str = "native-catalog-volume.bin";
	const CASE_FOLDED_LEDGER: &str = "native-hfsx-case-folded.json";
	const HFS_UNICODE_FOLDED: &str = "hfs_unicode_folded";

	let root = tempfile::tempdir().unwrap();
	let standard = root.path().join("hfsplus.raw");
	let case_folded = root.path().join("folded-hfsx.raw");
	let binary = root.path().join("binary-hfsx.raw");
	let mut bytes = hfsplus_fixture(STANDARD_CATALOG);
	fs::write(&standard, &bytes).unwrap();

	// Apply the already qualified native case-folded HFSX derivation.
	let ledger: serde_json::Value =
		serde_json::from_slice(&hfsplus_fixture(CASE_FOLDED_LEDGER)).unwrap();
	assert_eq!(ledger["base_fixture"], STANDARD_CATALOG);
	let patches = ledger["derivation"]["byte_overlays"].as_array().unwrap();
	assert!(!patches.is_empty());
	for patch in patches {
		let offset = usize::try_from(patch["offset"].as_u64().unwrap()).unwrap();
		let value = u8::try_from(patch["value"].as_u64().unwrap()).unwrap();
		assert_ne!(bytes[offset], value);
		bytes[offset] = value;
	}
	fs::write(&case_folded, bytes).unwrap();

	let equal = report(
		&semantic_diff(&standard, &case_folded, HFSPLUS, RAW_WITH_NATIVE_PATHS),
		EQUAL,
	);
	assert_eq!(equal["left"]["hfs_signature"], HFSPLUS);
	assert_eq!(equal["right"]["hfs_signature"], "hfsx");
	assert_eq!(equal["policy"]["native_name_profile"], HFS_UNICODE_FOLDED);
	assert_eq!(equal["changes"], json!([]));

	let bytes = materialize_sparse(&hfsplus_fixture(HFSX_FIXTURE), HFSX_VOLUME_BYTES);
	fs::write(&binary, bytes).unwrap();
	let incompatible = semantic_diff(&case_folded, &binary, HFSPLUS, RAW_WITH_NATIVE_PATHS);
	assert_error(&incompatible, "native filesystem path profiles differ");
}

#[test]
fn finder_info_compares_file_and_directory_catalog_fields_but_ignores_private_generation() {
	// Record starts in the captured nonjournaled HFSX catalog leaf.
	const CASES_FOLDER: usize = 2_376_166;
	const UPPERCASE_FILE: usize = 2_376_816;
	const LOWERCASE_FILE: usize = 2_377_356;
	const FOLDER_FINDER_FLAGS_OFFSET: usize = 8;
	const FINDER_INVISIBLE: u16 = 1 << 14;
	const WRITE_GENERATION_OFFSET: usize = 28;
	const NATIVE_WRITE_GENERATION: u32 = 2;
	const REVISED_WRITE_GENERATION: u32 = NATIVE_WRITE_GENERATION + 1;

	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("original.raw");
	let right = root.path().join("finder-info.raw");
	let mut bytes = materialize_sparse(&hfsplus_fixture(HFSX_FIXTURE), HFSX_VOLUME_BYTES);
	fs::write(&left, &bytes).unwrap();
	let folder = catalog_finder_info(&mut bytes, CASES_FOLDER, CATALOG_FOLDER);
	let flags = &mut folder[FOLDER_FINDER_FLAGS_OFFSET..][..size_of::<u16>()];
	assert_eq!(flags, 0_u16.to_be_bytes());
	flags.copy_from_slice(&FINDER_INVISIBLE.to_be_bytes());
	let file = catalog_finder_info(&mut bytes, UPPERCASE_FILE, CATALOG_FILE);
	let fields = &mut file[..FINDER_TYPE_AND_CREATOR.len()];
	assert_eq!(fields, [0; FINDER_TYPE_AND_CREATOR.len()]);
	fields.copy_from_slice(&FINDER_TYPE_AND_CREATOR);
	let file = catalog_finder_info(&mut bytes, LOWERCASE_FILE, CATALOG_FILE);
	let generation = &mut file[WRITE_GENERATION_OFFSET..][..size_of::<u32>()];
	assert_eq!(generation, NATIVE_WRITE_GENERATION.to_be_bytes());
	generation.copy_from_slice(&REVISED_WRITE_GENERATION.to_be_bytes());
	fs::write(&right, bytes).unwrap();

	for options in [BOTH_RAW, RAW_WITH_XATTRS] {
		let baseline = report(&semantic_diff(&left, &right, HFSPLUS, options), EQUAL);
		assert_eq!(baseline["changes"], json!([]));
		assert!(baseline["policy"].get(FINDER_INFO_FIELD).is_none());
	}
	for options in [BOTH_RAW, RAW_WITH_XATTRS] {
		let options = [options, &[FINDER_INFO_OPTION]].concat();
		let changed = report(&semantic_diff(&left, &right, HFSPLUS, &options), DIFFERENT);
		assert_eq!(
			changed["policy"][FINDER_INFO_FIELD]["record_bytes"],
			FINDER_INFO_BYTES
		);
		assert_eq!(
			changed["changes"],
			json!([
				{"path": ["Cases"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
				{"path": ["Cases", "Case.txt"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
			]),
			"{changed:#}"
		);
	}
}

#[test]
fn finder_info_reports_effective_hfsplus_hard_link_metadata_on_each_visible_alias() {
	const CHAINED_HARD_LINKS: &str = "native-chained-hardlinks.bin";
	const VOLUME_BYTES: usize = 16 * (1 << 20);
	const SHARED_INODE_RECORD: usize = 1_586_642;

	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("original.raw");
	let right = root.path().join("finder-info.raw");
	let mut bytes = materialize_sparse(&hfsplus_fixture(CHAINED_HARD_LINKS), VOLUME_BYTES);
	fs::write(&left, &bytes).unwrap();
	let stored = catalog_finder_info(&mut bytes, SHARED_INODE_RECORD, CATALOG_FILE);
	let fields = &mut stored[..FINDER_TYPE_AND_CREATOR.len()];
	assert_eq!(fields, [0; FINDER_TYPE_AND_CREATOR.len()]);
	fields.copy_from_slice(&FINDER_TYPE_AND_CREATOR);
	fs::write(&right, bytes).unwrap();

	let options = [BOTH_RAW, &[FINDER_INFO_OPTION]].concat();
	let changed = report(&semantic_diff(&left, &right, HFSPLUS, &options), DIFFERENT);
	assert_eq!(
		changed["changes"],
		json!([
			{"path": ["nested", "three"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
			{"path": ["one"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
			{"path": ["two"], "kind": "changed", "fields": [FINDER_INFO_FIELD]},
		]),
		"{changed:#}"
	);
}

fn catalog_finder_info(image: &mut [u8], start: usize, kind: u16) -> &mut [u8] {
	let record = &mut image[start..];
	assert_eq!(record[..CATALOG_RECORD_KIND_BYTES], kind.to_be_bytes());
	&mut record[CATALOG_FINDER_INFO_OFFSET..][..FINDER_INFO_BYTES]
}

#[test]
fn compares_the_native_contiguous_and_fragmented_hfsplus_attribute_forks() {
	const STRUCTURE: &str = "native-attribute-structure.bin";
	const FORKS: &str = "native-attribute-forks.bin";
	const VOLUME_BYTES: usize = 16 * (1 << 20);
	const CONTIGUOUS_AT: usize = 3_026_944;
	// This continuation extent starts after the first comparison chunk.
	const FRAGMENTED_AT: usize = 3_158_016;
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("left.raw");
	let right = root.path().join("right.raw");
	// Both captures document ranges of the same native image. The structure
	// supplies the catalog independently of the attribute tree and payloads.
	let mut bytes = materialize_sparse(&hfsplus_fixture(STRUCTURE), VOLUME_BYTES);
	overlay_sparse(&mut bytes, &hfsplus_fixture(FORKS));
	fs::write(&left, &bytes).unwrap();

	assert_eq!(bytes[CONTIGUOUS_AT], b'0');
	assert_eq!(bytes[FRAGMENTED_AT], 20);
	bytes[CONTIGUOUS_AT] ^= 1;
	bytes[FRAGMENTED_AT] ^= 1;
	fs::write(&right, bytes).unwrap();

	let baseline = report(&semantic_diff(&left, &right, HFSPLUS, BOTH_RAW), EQUAL);
	assert_eq!(baseline["changes"], json!([]));
	let actual = report(
		&semantic_diff(&left, &right, HFSPLUS, RAW_WITH_XATTRS),
		DIFFERENT,
	);
	assert_eq!(
		actual["changes"],
		json!([{
			"path": ["sample"],
			"kind": "changed",
			"fields": ["extended_attributes"],
			"xattrs": [
				{ "name": "org.pomologie.contiguous", "kind": "changed" },
				{ "name": "org.pomologie.fragmented", "kind": "changed" },
			],
		}]),
		"{actual:#}",
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
pub(super) fn build(root: &Path, name: &str, entries: &str) -> PathBuf {
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
