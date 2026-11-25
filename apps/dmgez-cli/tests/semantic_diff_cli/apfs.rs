use std::{
	fs,
	io::{Read, Seek, SeekFrom, Write},
	path::Path,
	process::Command,
};

use serde_json::json;

use super::{
	BINARY, DIFFERENT, EQUAL, ERROR,
	fixture::{
		HARD_LINK_CHECKPOINT_XID, HARD_LINK_FIXTURE, HARD_LINK_VOLUME_OID, HARD_LINK_VOLUME_UUID,
		NATIVE_BLOCK_BYTES, NATIVE_VOLUME_BYTES, SPARSE_FIXTURE, materialize,
	},
	native_apfs_mutations::advertise_incomplete_checkpoint,
	report, semantic_diff,
};

const APFS: &str = "apfs";
const BOTH_RAW: &[&str] = &["--left-raw", "--right-raw"];
const DATA_BLOCK: u64 = 201;
const RESOURCE_BLOCK: u64 = 204;
const DATA_PATTERN: &[u8] = b"APFS shared data fork\n";
const RESOURCE_PATTERN: &[u8] = b"APFS shared resource fork\n";

#[test]
fn same_native_volume_compares_equal_across_disk_layouts() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("whole.raw");
	let right = root.path().join("partition.raw");
	materialize(&left, HARD_LINK_FIXTURE, &[]);
	materialize(&right, HARD_LINK_FIXTURE, &mbr_prefix());

	let physical = Command::new(BINARY)
		.arg("diff")
		.arg(&left)
		.arg(&right)
		.args(["--mode", "physical", "--json"])
		.output()
		.unwrap();
	assert_eq!(physical.status.code(), Some(DIFFERENT), "{physical:?}");

	let output = semantic_diff(
		&left,
		&right,
		APFS,
		&[
			"--left-raw",
			"--right-raw",
			"--right-partition",
			"1",
			"--left-volume",
			HARD_LINK_VOLUME_UUID,
			"--right-volume",
			HARD_LINK_VOLUME_OID,
			"--left-xid",
			HARD_LINK_CHECKPOINT_XID,
			"--right-xid",
			HARD_LINK_CHECKPOINT_XID,
		],
	);
	let report = report(&output, EQUAL);
	assert_eq!(report["outcome"], "equal", "{report:#}");
	assert_eq!(report["changes"], json!([]));
	assert_eq!(report["policy"]["paths"], "exact_stored_text");
	assert_eq!(report["policy"]["contents"], "exact_bytes");
	let excluded = report["policy"]["excluded"].as_array().unwrap();
	for field in ["timestamps", "physical_container_layout", "signatures"] {
		assert!(excluded.contains(&json!(field)), "{report:#}");
	}
	assert_eq!(report["filesystem"], APFS);
	assert_eq!(report["left"]["window"], json!({"kind": "whole_disk"}));
	assert_eq!(
		report["right"]["window"],
		json!({"kind": "partition", "id": {"scheme": "mbr", "slot": 1}}),
	);
	let identity = json!({
		"uuid": HARD_LINK_VOLUME_UUID.to_ascii_lowercase(),
		"object_id": HARD_LINK_VOLUME_OID,
		"checkpoint_xid": HARD_LINK_CHECKPOINT_XID,
	});
	assert_eq!(report["left"]["apfs_volume"], identity);
	assert_eq!(report["right"]["apfs_volume"], identity);
}

#[cfg(feature = "build")]
#[test]
fn raw_and_udzo_apfs_compare_equal_despite_different_physical_bytes() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("native.raw");
	let right = root.path().join("wrapped.dmg");
	materialize(&left, HARD_LINK_FIXTURE, &[]);
	let wrapped = Command::new(BINARY)
		.arg("wrap-raw")
		.arg(&left)
		.args(["--format", "udzo", "--output"])
		.arg(&right)
		.output()
		.unwrap();
	assert!(wrapped.status.success(), "{wrapped:?}");
	assert!(fs::metadata(&left).unwrap().len() > fs::metadata(&right).unwrap().len());

	let output = semantic_diff(&left, &right, APFS, &["--left-raw"]);
	let report = report(&output, EQUAL);
	assert_eq!(report["outcome"], "equal", "{report:#}");
	assert_eq!(report["left"]["input"], "raw");
	assert_eq!(report["right"]["input"], "udif");
	assert_eq!(report["changes"], json!([]));
}

#[test]
fn distinguishes_one_changed_fork_on_each_shared_alias() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("left.raw");
	materialize(&left, HARD_LINK_FIXTURE, &[]);
	for (block, original, field) in [
		(DATA_BLOCK, DATA_PATTERN, "data_fork"),
		(RESOURCE_BLOCK, RESOURCE_PATTERN, "resource_fork"),
	] {
		let right = root.path().join(field).with_extension("raw");
		materialize(&right, HARD_LINK_FIXTURE, &[]);
		change_native_fork_byte(&right, block, original);

		let output = semantic_diff(&left, &right, APFS, BOTH_RAW);
		let report = report(&output, DIFFERENT);
		assert_eq!(report["outcome"], "different", "{report:#}");
		let changes = report["changes"].as_array().unwrap();
		assert_eq!(changes.len(), 3, "{report:#}");
		for path in [
			json!(["Docs", "alias.bin"]),
			json!(["More", "Écho.bin"]),
			json!(["primary.bin"]),
		] {
			assert!(
				changes.iter().any(|change| change["path"] == path
					&& change["kind"] == "changed"
					&& change["fields"] == json!([field])),
				"missing {path}: {report:#}",
			);
		}
	}
}

#[test]
fn compares_additions_and_removals_between_different_native_volumes() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("links.raw");
	let right = root.path().join("sparse.raw");
	materialize(&left, HARD_LINK_FIXTURE, &[]);
	materialize(&right, SPARSE_FIXTURE, &[]);

	let output = semantic_diff(&left, &right, APFS, BOTH_RAW);
	let report = report(&output, DIFFERENT);
	assert_eq!(report["outcome"], "different", "{report:#}");
	let changes = report["changes"].as_array().unwrap();
	for (path, kind) in [
		(json!(["primary.bin"]), "removed"),
		(json!(["Docs", "alias.bin"]), "removed"),
		(json!(["sparse.bin"]), "added"),
		(json!(["all-hole.bin"]), "added"),
	] {
		assert!(
			changes
				.iter()
				.any(|change| change["path"] == path && change["kind"] == kind),
			"missing {kind} {path}: {report:#}",
		);
	}
}

#[test]
fn exhausting_the_payload_budget_has_no_equality_report() {
	let root = tempfile::tempdir().unwrap();
	let image = root.path().join("native.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	let output = semantic_diff(
		&image,
		&image,
		APFS,
		&["--left-raw", "--right-raw", "--max-bytes", "1"],
	);
	assert_eq!(output.status.code(), Some(ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains("budget"), "{stderr}");
}

#[test]
fn a_recovered_apfs_view_requires_explicit_checkpoint_selection() {
	let root = tempfile::tempdir().unwrap();
	let left = root.path().join("clean.raw");
	let right = root.path().join("recovered.raw");
	materialize(&left, HARD_LINK_FIXTURE, &[]);
	materialize(&right, HARD_LINK_FIXTURE, &[]);
	advertise_incomplete_checkpoint(&right);

	let output = semantic_diff(&left, &right, APFS, BOTH_RAW);
	assert_eq!(output.status.code(), Some(ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		stderr.contains("right") && stderr.contains("checkpoint"),
		"{stderr}"
	);

	let output = semantic_diff(
		&left,
		&right,
		APFS,
		&[
			"--left-raw",
			"--right-raw",
			"--right-xid",
			HARD_LINK_CHECKPOINT_XID,
		],
	);
	let report = report(&output, EQUAL);
	assert_eq!(report["outcome"], "equal", "{report:#}");
	assert_eq!(
		report["right"]["apfs_volume"]["checkpoint_xid"],
		HARD_LINK_CHECKPOINT_XID
	);
}

// The native hard-link capture stores both forks outside metadata objects.
// Assert the bytes from the recorded Apple oracle before changing a single
// data octet, leaving its stream length and every checksum intact.
fn change_native_fork_byte(path: &Path, block: u64, pattern: &[u8]) {
	let mut file = fs::OpenOptions::new()
		.read(true)
		.write(true)
		.open(path)
		.unwrap();
	let offset = block * NATIVE_BLOCK_BYTES as u64;
	file.seek(SeekFrom::Start(offset)).unwrap();
	let mut observed = vec![0; pattern.len()];
	file.read_exact(&mut observed).unwrap();
	assert_eq!(observed, pattern);
	file.seek(SeekFrom::Start(offset)).unwrap();
	file.write_all(&[pattern[0] ^ 1]).unwrap();
}

fn mbr_prefix() -> Vec<u8> {
	const SECTOR_BYTES: usize = 512;
	const TABLE_OFFSET: usize = 446;
	const TYPE_OFFSET: usize = TABLE_OFFSET + 4;
	const FIRST_SECTOR_OFFSET: usize = TABLE_OFFSET + 8;
	const SECTORS_OFFSET: usize = TABLE_OFFSET + 12;
	const SIGNATURE_OFFSET: usize = 510;
	const SIGNATURE: [u8; 2] = [0x55, 0xaa];
	const NON_DOS_DATA_PARTITION: u8 = 0xda;
	const FIRST_LBA: u32 = 2048;

	let mut prefix = vec![0; FIRST_LBA as usize * SECTOR_BYTES];
	let sectors = u32::try_from(NATIVE_VOLUME_BYTES / SECTOR_BYTES as u64).unwrap();
	prefix[TYPE_OFFSET] = NON_DOS_DATA_PARTITION;
	prefix[FIRST_SECTOR_OFFSET..][..size_of::<u32>()].copy_from_slice(&FIRST_LBA.to_le_bytes());
	prefix[SECTORS_OFFSET..][..size_of::<u32>()].copy_from_slice(&sectors.to_le_bytes());
	prefix[SIGNATURE_OFFSET..][..SIGNATURE.len()].copy_from_slice(&SIGNATURE);
	prefix
}
