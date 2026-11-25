use std::{
	fs,
	os::unix::fs::{MetadataExt, PermissionsExt},
	path::{Path, PathBuf},
	process::Command,
};

use serde_json::Value;

use super::{BINARY, extract, failure, report, success};

const PROJECT_FILE: &str = "extract.toml";
const IMAGE_FILE: &str = "extract.dmg";
const PAYLOAD_FILE: &str = "payload.bin";
const SMALL_PAYLOAD: &[u8] = b"published complete payload\x00\xff\n";
const LINK_SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Extraction Witness"
output = "extract.dmg"
format = "udro"
size = "1MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = false
[[contents]]
source = "payload.bin"
destination = "/Links/Before.txt"
[[contents]]
type = "symlink"
destination = "/Links/Escape"
target = "/Outside"
[[contents]]
source = "payload.bin"
destination = "/Unsafe/Before.txt"
[[contents]]
source = "payload.bin"
destination = "/Unsafe/NUL.txt"
[[contents]]
source = "payload.bin"
destination = "/Outside/Secret"
"#;
const PARTIAL_SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Extraction Failure Witness"
output = "extract.dmg"
format = "udzo"
size = "12MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = false
[[contents]]
source = "payload.bin"
destination = "/Tree/first.bin"
[[contents]]
source = "large.bin"
destination = "/Tree/second.bin"
"#;
const HARD_LINK_SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Extraction Hard-link Witness"
output = "extract.dmg"
format = "udro"
size = "1MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = false
preserve_hard_links = true
[[contents]]
source = "payload.bin"
destination = "/Tree/Original.bin"
[[contents]]
source = "alias.bin"
destination = "/Tree/Nested/Alias.bin"
[[contents]]
source = "plain.bin"
destination = "/Outside/Only.txt"
"#;
const COPY_ROOT: &str = "/Tree";
const FIRST_FILE: &str = "first.bin";
const SECOND_FILE: &str = "second.bin";
const LARGE_FILE: &str = "large.bin";
const MEBIBYTE: usize = 1 << 20;
const LARGE_PAYLOAD_BYTES: usize = 6 * MEBIBYTE;
const LARGE_PAYLOAD_FILL: u8 = 0x8b;
const TARGET_LOGICAL_OFFSET: u64 = 3 * MEBIBYTE as u64;
const INVALID_ZLIB_HEADER: u8 = 0;
const EXECUTABLE_SOURCE_PERMISSIONS: u32 = 0o751;
const PERMISSION_BITS: u32 = 0o7777;
const OWNER_ONLY_FILE: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

fn build(root: &Path, settings: &str) -> PathBuf {
	let project = root.join(PROJECT_FILE);
	fs::write(root.join(PAYLOAD_FILE), SMALL_PAYLOAD).unwrap();
	fs::write(&project, settings).unwrap();
	success(
		Command::new(BINARY)
			.arg("build")
			.arg(project)
			.output()
			.unwrap(),
	);
	root.join(IMAGE_FILE)
}

#[test]
fn rejects_image_symlinks_and_reserved_names_before_creating_the_destination() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = build(&root, LINK_SETTINGS);
	let source = fs::read(&image).unwrap();
	let output = root.join("new");
	for (path, expected) in [("/Links", "Symlink"), ("/Unsafe", "reserved Win32 device")] {
		for args in [&["--data-only"][..], &["--data-only", "--dry-run"][..]] {
			let diagnostic = failure(extract(&image, path, &output, args));
			assert!(diagnostic.contains(expected), "{diagnostic}");
			assert!(!output.exists());
		}
	}
	let diagnostic = failure(extract(
		&image,
		"/Links/Escape",
		&output,
		&["--fork", "data"],
	));
	assert!(diagnostic.contains("regular file"), "{diagnostic}");
	failure(extract(
		&image,
		"/Links/Escape/Secret",
		&output,
		&["--fork", "data"],
	));
	assert!(!output.exists());
	assert_eq!(fs::read(&image).unwrap(), source);
	assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
}

#[test]
fn reports_hard_link_materialization_and_never_restores_execute_permissions() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let payload = root.join(PAYLOAD_FILE);
	fs::write(&payload, SMALL_PAYLOAD).unwrap();
	fs::set_permissions(
		&payload,
		fs::Permissions::from_mode(EXECUTABLE_SOURCE_PERMISSIONS),
	)
	.unwrap();
	fs::hard_link(&payload, root.join("alias.bin")).unwrap();
	fs::write(root.join("plain.bin"), b"outside the selected image tree").unwrap();
	let image = build(&root, HARD_LINK_SETTINGS);
	let output = root.join("tree");
	let source = fs::read(&image).unwrap();

	let consulted = success(
		Command::new(BINARY)
			.arg("stat")
			.arg(&image)
			.args(["/Tree/Original.bin", "--json"])
			.output()
			.unwrap(),
	);
	let metadata: Value = serde_json::from_slice(&consulted).unwrap();
	assert_eq!(metadata["hard_link_count"], 2);
	assert_eq!(
		u32::try_from(metadata["mode"].as_u64().unwrap()).unwrap() & PERMISSION_BITS,
		EXECUTABLE_SOURCE_PERMISSIONS
	);

	let executed = success(extract(
		&image,
		COPY_ROOT,
		&output,
		&["--data-only", "--json"],
	));
	let executed = report(&executed, "complete", COPY_ROOT, &output);
	assert_eq!(executed["plan"]["hard_links_materialized"], 2);
	assert_eq!(executed["plan"]["files"], 2);
	assert_eq!(executed["progress"]["files_published"], 2);
	assert_eq!(executed["progress"]["directories_created"], 2);
	assert_eq!(
		executed["progress"]["bytes_published"],
		2 * SMALL_PAYLOAD.len()
	);
	let omitted = executed["plan"]["omitted"].as_array().unwrap();
	for property in [
		"other_forks",
		"permissions",
		"extended_attributes",
		"hard_link_topology",
	] {
		assert!(omitted.iter().any(|item| item == property), "{omitted:?}");
	}
	let original = output.join("Original.bin");
	let alias = output.join("Nested/Alias.bin");
	assert_ne!(
		fs::metadata(&original).unwrap().ino(),
		fs::metadata(&alias).unwrap().ino()
	);
	for file in [original, alias] {
		assert_eq!(fs::read(&file).unwrap(), SMALL_PAYLOAD);
		assert_eq!(
			fs::metadata(file).unwrap().mode() & PERMISSION_BITS,
			OWNER_ONLY_FILE
		);
	}
	for directory in [&output, &output.join("Nested")] {
		assert_eq!(
			fs::metadata(directory).unwrap().mode() & PERMISSION_BITS,
			OWNER_ONLY_DIRECTORY
		);
	}
	assert_eq!(fs::read_dir(&output).unwrap().count(), 2);
	assert!(!output.join("Outside").exists());
	assert_eq!(fs::read(&image).unwrap(), source);
}

#[test]
fn reports_a_late_payload_error_with_only_confirmed_publications() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	fs::write(
		root.join(LARGE_FILE),
		vec![LARGE_PAYLOAD_FILL; LARGE_PAYLOAD_BYTES],
	)
	.unwrap();
	let image = build(&root, PARTIAL_SETTINGS);
	let output = root.join("tree");
	let original = success(extract(
		&image,
		COPY_ROOT,
		&output,
		&["--data-only", "--dry-run", "--json"],
	));
	let original = report(&original, "planned", COPY_ROOT, &output);

	let changed = corrupt_zlib_run(&image);
	let planned = success(extract(
		&image,
		COPY_ROOT,
		&output,
		&["--data-only", "--dry-run", "--json"],
	));
	let planned = report(&planned, "planned", COPY_ROOT, &output);
	assert_eq!(planned["plan"], original["plan"]);
	assert!(!output.exists());

	let attempted = extract(&image, COPY_ROOT, &output, &["--data-only", "--json"]);
	assert!(!attempted.status.success(), "{attempted:?}");
	let partial = report(&attempted.stdout, "partial", COPY_ROOT, &output);
	assert_eq!(partial["plan"], planned["plan"]);
	assert_eq!(partial["progress"]["files_published"], 1);
	assert_eq!(partial["progress"]["directories_created"], 1);
	assert_eq!(partial["progress"]["bytes_published"], SMALL_PAYLOAD.len());
	assert!(!partial["error"].is_null());
	assert_eq!(fs::read(output.join(FIRST_FILE)).unwrap(), SMALL_PAYLOAD);
	assert!(!output.join(SECOND_FILE).exists());
	assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
	assert_eq!(fs::read(&image).unwrap(), changed);
}

fn corrupt_zlib_run(image: &Path) -> Vec<u8> {
	let stdout = success(
		Command::new(BINARY)
			.arg("inspect")
			.arg(image)
			.args(["--json", "--run-limit", "128"])
			.output()
			.unwrap(),
	);
	let document: Value = serde_json::from_slice(&stdout).unwrap();
	let run = document["run_page"]["runs"]
		.as_array()
		.unwrap()
		.iter()
		.find(|run| decimal(&run["logical"]["offset_bytes"]) == TARGET_LOGICAL_OFFSET)
		.unwrap();
	assert_eq!(run["encoding"]["kind"], "zlib");
	assert_eq!(decimal(&run["logical"]["length_bytes"]), MEBIBYTE as u64);
	let offset = usize::try_from(decimal(&run["physical"]["offset_bytes"])).unwrap();
	let mut bytes = fs::read(image).unwrap();
	assert_ne!(bytes[offset], INVALID_ZLIB_HEADER);
	bytes[offset] = INVALID_ZLIB_HEADER;
	fs::write(image, &bytes).unwrap();
	bytes
}

fn decimal(number: &Value) -> u64 {
	number.as_str().unwrap().parse().unwrap()
}
