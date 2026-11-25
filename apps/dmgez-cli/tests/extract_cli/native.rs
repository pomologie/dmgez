use std::{
	env, fs,
	os::unix::fs::{MetadataExt, symlink},
	path::{Path, PathBuf},
	process::Command,
};

use super::{BINARY, extract, failure, report, success};

const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";
pub(super) const APM_IMAGE: &[u8] =
	include_bytes!("../../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
pub(super) const APM_HFS_SLOT: &str = "2";
const APM_METADATA_SLOT: &str = "1";
const HARD_LINK_FIXTURE: &str = "native-chained-hardlinks.bin";
const HARD_LINK_VOLUME_BYTES: usize = 16 * 1024 * 1024;
const HFSX_FIXTURE: &str = "native-hfsx.bin";
const HFSX_VOLUME_BYTES: usize = 24 * 1024 * 1024;
const OPTICAL_FIXTURE: &str = "optical-volume.bin";
const OPTICAL_PATH: &str = "/hello.txt";
const OPTICAL_DATA: &[u8] = b"pomologie fixture\n";
const RESOURCE_FIXTURE: &str = "native-decmpfs-zlib.bin";
const RESOURCE_VOLUME_BYTES: usize = 64 * 1024 * 1024;
const RESOURCE_PATH: &str = "/resource";
// The sibling's native-decmpfs-zlib.json records the entire stored fork slice.
const RESOURCE_DATA_OFFSET: usize = 12_066_816;
const RESOURCE_DATA_BYTES: usize = 1_115;
const PERMISSION_BITS: u32 = 0o7777;
const OWNER_READ_WRITE: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

fn native_fixture(name: &str, expected_length: usize) -> Vec<u8> {
	materialize(&fixture_bytes(name), expected_length)
}

fn fixture_bytes(name: &str) -> Vec<u8> {
	let directory = env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies HFS+ fixtures");
	let path = PathBuf::from(directory).join(name);
	fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn materialize(mut encoded: &[u8], expected_length: usize) -> Vec<u8> {
	fn read_field<const WIDTH: usize>(input: &mut &[u8]) -> [u8; WIDTH] {
		let (field, rest) = input.split_first_chunk().unwrap();
		*input = rest;
		*field
	}

	// The sibling fixture ledgers describe the exact sparse encoding and
	// native qualification. Extract receives the reconstructed raw disk.
	let length = usize::try_from(u64::from_be_bytes(read_field(&mut encoded))).unwrap();
	assert_eq!(length, expected_length);
	let runs = u16::from_be_bytes(read_field(&mut encoded));
	let mut bytes = vec![0; length];
	for _ in 0..runs {
		let start = usize::try_from(u64::from_be_bytes(read_field(&mut encoded))).unwrap();
		let count = usize::try_from(u32::from_be_bytes(read_field(&mut encoded))).unwrap();
		let (content, remaining) = encoded.split_at(count);
		bytes[start..][..count].copy_from_slice(content);
		encoded = remaining;
	}
	assert!(encoded.is_empty());
	bytes
}

fn optical(root: &Path) -> (PathBuf, Vec<u8>) {
	let image = root.join("native-optical.raw");
	let bytes = fixture_bytes(OPTICAL_FIXTURE);
	fs::write(&image, &bytes).unwrap();
	(image, bytes)
}

fn permissions(path: &Path) -> u32 {
	fs::metadata(path).unwrap().mode() & PERMISSION_BITS
}

#[test]
fn dry_run_then_copy_the_exact_native_data_and_resource_forks() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let (image, source) = optical(&root);
	let resource_image = root.join("native-resource.raw");
	let resource_source = native_fixture(RESOURCE_FIXTURE, RESOURCE_VOLUME_BYTES);
	fs::write(&resource_image, &resource_source).unwrap();

	for (image, path, choice, kind, bytes) in [
		(&image, OPTICAL_PATH, "data", "data_fork", OPTICAL_DATA),
		(
			&resource_image,
			RESOURCE_PATH,
			"resource",
			"resource_fork",
			&resource_source[RESOURCE_DATA_OFFSET..][..RESOURCE_DATA_BYTES],
		),
	] {
		let output = root.join(format!("{choice}.bin"));
		let planned = success(extract(
			image,
			path,
			&output,
			&["--raw", "--fork", choice, "--dry-run", "--json"],
		));
		let planned = report(&planned, "planned", path, &output);
		assert_eq!(planned["plan"]["selection"], kind);
		assert_eq!(planned["plan"]["files"], 1);
		assert_eq!(planned["plan"]["bytes"], bytes.len());
		assert!(planned["progress"].is_null());
		assert!(!output.exists());

		let executed = success(extract(
			image,
			path,
			&output,
			&["--raw", "--fork", choice, "--json"],
		));
		let executed = report(&executed, "complete", path, &output);
		assert_eq!(executed["plan"], planned["plan"]);
		assert_eq!(executed["progress"]["files_published"], 1);
		assert_eq!(executed["progress"]["bytes_published"], bytes.len());
		assert_eq!(fs::read(&output).unwrap(), bytes);
		assert_eq!(permissions(&output), OWNER_READ_WRITE);
	}

	let empty = root.join("empty-resource.bin");
	success(extract(
		&image,
		OPTICAL_PATH,
		&empty,
		&["--raw", "--fork", "resource", "--max-bytes", "0"],
	));
	assert!(fs::read(&empty).unwrap().is_empty());
	assert_eq!(fs::read(&image).unwrap(), source);
	assert_eq!(fs::read(&resource_image).unwrap(), resource_source);
	assert_eq!(fs::read_dir(&root).unwrap().count(), 5);
}

#[test]
fn data_only_tree_obeys_the_total_byte_budget_before_creating_the_root() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let (image, source) = optical(&root);
	let output = root.join("tree");
	let too_small = (OPTICAL_DATA.len() - 1).to_string();
	let exact = OPTICAL_DATA.len().to_string();
	for args in [
		&["--raw", "--data-only", "--max-bytes", &too_small][..],
		&[
			"--raw",
			"--data-only",
			"--max-bytes",
			&too_small,
			"--dry-run",
		][..],
	] {
		let diagnostic = failure(extract(&image, "/", &output, args));
		assert!(diagnostic.contains("total byte budget"), "{diagnostic}");
		assert!(!output.exists());
	}
	let planned = success(extract(
		&image,
		"/",
		&output,
		&[
			"--raw",
			"--data-only",
			"--dry-run",
			"--json",
			"--max-bytes",
			&exact,
		],
	));
	let plan = report(&planned, "planned", "/", &output);
	assert_eq!(plan["plan"]["selection"], "data_tree");
	assert_eq!(plan["plan"]["files"], 1);
	assert_eq!(plan["plan"]["directories"], 1);
	assert_eq!(plan["plan"]["bytes"], OPTICAL_DATA.len());
	assert!(!output.exists());
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

	let executed = success(extract(
		&image,
		"/",
		&output,
		&["--raw", "--data-only", "--json", "--max-bytes", &exact],
	));
	let executed = report(&executed, "complete", "/", &output);
	assert_eq!(executed["plan"], plan["plan"]);
	assert_eq!(executed["progress"]["files_published"], 1);
	assert_eq!(executed["progress"]["directories_created"], 1);
	assert_eq!(executed["progress"]["bytes_published"], OPTICAL_DATA.len());
	assert_eq!(fs::read(output.join("hello.txt")).unwrap(), OPTICAL_DATA);
	assert_eq!(permissions(&output.join("hello.txt")), OWNER_READ_WRITE);
	assert_eq!(permissions(&output), OWNER_ONLY_DIRECTORY);
	assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
	assert_eq!(fs::read(&image).unwrap(), source);
}

#[test]
fn respects_native_partition_selection_and_never_treats_raw_input_as_udif() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = root.join("apm.dmg");
	let output = root.join("apm-tree");
	fs::write(&image, APM_IMAGE).unwrap();
	for slot in [APM_METADATA_SLOT, "99"] {
		failure(extract(
			&image,
			"/",
			&output,
			&["--partition", slot, "--data-only", "--dry-run"],
		));
		assert!(!output.exists());
	}
	let planned = success(extract(
		&image,
		"/",
		&output,
		&[
			"--partition",
			APM_HFS_SLOT,
			"--data-only",
			"--dry-run",
			"--json",
		],
	));
	let planned = report(&planned, "planned", "/", &output);
	assert_eq!(planned["plan"]["directories"], 1);
	assert_eq!(planned["plan"]["files"], 0);
	assert!(!output.exists());
	assert_eq!(fs::read(&image).unwrap(), APM_IMAGE);

	let (raw, source) = optical(&root);
	failure(extract(&raw, OPTICAL_PATH, &output, &["--fork", "data"]));
	assert!(!output.exists());
	assert_eq!(fs::read(&raw).unwrap(), source);
}

#[test]
fn refuses_alias_paths_and_conflicts_without_changing_any_destination() {
	const SENTINEL: &[u8] = b"existing destination bytes";
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let (image, source) = optical(&root);
	let existing = root.join("existing");
	let missing = root.join("missing");
	let dangling = root.join("dangling");
	let parent_alias = root.join("alias");
	let nested_output = parent_alias.join("new");
	let real_parent = root.join("parent");
	fs::write(&existing, SENTINEL).unwrap();
	fs::create_dir(&real_parent).unwrap();
	symlink(&missing, &dangling).unwrap();
	symlink(&real_parent, &parent_alias).unwrap();

	for output in [&existing, &dangling, &nested_output, &image] {
		for args in [
			&["--raw", "--fork", "data"][..],
			&["--raw", "--fork", "data", "--dry-run"][..],
		] {
			failure(extract(&image, OPTICAL_PATH, output, args));
		}
	}
	for path in [
		"hello.txt",
		"//hello.txt",
		"/hello.txt/",
		"/../hello.txt",
		"/./hello.txt",
	] {
		failure(extract(
			&image,
			path,
			&missing,
			&["--raw", "--fork", "data"],
		));
	}
	failure(extract(&image, "/", &missing, &["--raw", "--fork", "data"]));
	failure(extract(
		&image,
		OPTICAL_PATH,
		&missing,
		&["--raw", "--data-only"],
	));
	failure(extract(&image, OPTICAL_PATH, &missing, &["--raw"]));
	assert_eq!(fs::read(&existing).unwrap(), SENTINEL);
	assert_eq!(fs::read_link(&dangling).unwrap(), missing);
	assert_eq!(fs::read_link(&parent_alias).unwrap(), real_parent);
	assert!(!missing.exists());
	assert_eq!(fs::read_dir(&real_parent).unwrap().count(), 0);
	assert_eq!(fs::read_dir(&root).unwrap().count(), 5);
	assert_eq!(fs::read(&image).unwrap(), source);
}

#[test]
fn resolves_a_relative_output_from_the_explicit_working_directory() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let (image, source) = optical(&root);
	let copy = "relative.bin";
	let escape = Command::new(BINARY)
		.current_dir(&root)
		.arg("extract")
		.arg(&image)
		.args([
			OPTICAL_PATH,
			"--raw",
			"--fork",
			"data",
			"--output",
			"missing/../escape.bin",
		])
		.output()
		.unwrap();
	failure(escape);
	assert!(!root.join("escape.bin").exists());
	let output = Command::new(BINARY)
		.current_dir(&root)
		.arg("extract")
		.arg(&image)
		.args([OPTICAL_PATH, "--raw", "--fork", "data", "--output", copy])
		.output()
		.unwrap();
	success(output);
	assert_eq!(fs::read(root.join(copy)).unwrap(), OPTICAL_DATA);
	assert_eq!(fs::read(&image).unwrap(), source);
}

#[test]
fn rejects_native_hfsx_names_outside_the_portable_profile_before_creating_the_root() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = root.join("native-hfsx.raw");
	let bytes = native_fixture(HFSX_FIXTURE, HFSX_VOLUME_BYTES);
	fs::write(&image, &bytes).unwrap();
	let output = root.join("case-collision");
	let diagnostic = failure(extract(
		&image,
		"/Cases",
		&output,
		&["--raw", "--data-only", "--dry-run"],
	));
	assert!(diagnostic.contains("lossless encoding"), "{diagnostic}");
	assert!(!output.exists());
	let diagnostic = failure(extract(
		&image,
		"/Cases",
		&output,
		&["--raw", "--data-only"],
	));
	assert!(diagnostic.contains("lossless encoding"), "{diagnostic}");
	assert!(!output.exists());
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
	assert_eq!(fs::read(&image).unwrap(), bytes);
}

#[test]
fn fails_closed_when_a_native_capture_lacks_its_attribute_tree() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = root.join("incomplete-attributes.raw");
	let bytes = native_fixture(HARD_LINK_FIXTURE, HARD_LINK_VOLUME_BYTES);
	fs::write(&image, &bytes).unwrap();
	let output = root.join("new");

	// This native capture retains catalog data, but omits the header of the
	// attributes B-tree. An unreadable quarantine state is not absence.
	for args in [
		&["--raw", "--fork", "data", "--dry-run"][..],
		&["--raw", "--fork", "data"][..],
	] {
		let diagnostic = failure(extract(&image, "/one", &output, args));
		assert!(
			diagnostic.contains("source quarantine state"),
			"{diagnostic}"
		);
		assert!(!output.exists());
	}
	assert_eq!(fs::read(&image).unwrap(), bytes);
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}
