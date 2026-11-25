use std::fs;

use serde_json::Value;

use super::{
	browse::{
		FIXTURE_HARD_LINKS, HARD_LINK_DATA, HARD_LINK_RESOURCE, HARD_LINK_VOLUME_BYTES, report,
		success,
	},
	invoke,
	native_hfsplus::{hfsplus_fixture, materialize_sparse},
};

const WRITER_CHUNK_BYTES: usize = 1024 * 1024;
// Outside the native fixture's retained HFS+ catalog and data forks.
const UNRELATED_LOGICAL_OFFSET: usize = 12 * WRITER_CHUNK_BYTES;
const COMPRESSIBLE_WITNESS: u8 = 0xa6;
const INVALID_ZLIB_HEADER: u8 = 0;
// The native hard-link ledger records both inode data forks and the first
// inode's resource fork within one writer chunk.
const NATIVE_FORK_OFFSETS: [u64; 3] = [3_022_848, 3_026_944, 3_031_040];
const SECOND_INODE_DATA: &[u8] = b"pomologie second group\n";

#[test]
fn query_reads_leave_an_unrelated_corrupt_compressed_run_unopened() {
	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("native.raw");
	let image = directory.path().join("native.dmg");
	let mut bytes =
		materialize_sparse(&hfsplus_fixture(FIXTURE_HARD_LINKS), HARD_LINK_VOLUME_BYTES);
	let unrelated = &mut bytes[UNRELATED_LOGICAL_OFFSET..][..WRITER_CHUNK_BYTES];
	assert!(unrelated.iter().all(|&byte| byte == 0));
	unrelated.fill(COMPRESSIBLE_WITNESS);
	fs::write(&raw, bytes).unwrap();
	success(invoke(
		"wrap-raw",
		&raw,
		&["--format", "udzo", "-o", image.to_str().unwrap()],
	));

	let inspected: Value = serde_json::from_slice(&success(invoke(
		"inspect",
		&image,
		&["--json", "--run-limit", "128"],
	)))
	.unwrap();
	let runs = inspected["run_page"]["runs"].as_array().unwrap();
	assert!(runs.iter().any(|run| {
		let start = decimal(&run["logical"]["offset_bytes"]);
		let end = start + decimal(&run["logical"]["length_bytes"]);
		run["encoding"]["kind"] == "zlib"
			&& NATIVE_FORK_OFFSETS
				.iter()
				.all(|offset| (start..end).contains(offset))
	}));
	let target = runs
		.iter()
		.find(|run| decimal(&run["logical"]["offset_bytes"]) == UNRELATED_LOGICAL_OFFSET as u64)
		.unwrap();
	assert_eq!(target["encoding"]["kind"], "zlib");
	assert_eq!(
		target["logical"]["length_bytes"],
		WRITER_CHUNK_BYTES.to_string()
	);
	let offset: usize = target["physical"]["offset_bytes"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap();
	let mut image_bytes = fs::read(&image).unwrap();
	assert_ne!(image_bytes[offset], INVALID_ZLIB_HEADER);
	image_bytes[offset] = INVALID_ZLIB_HEADER;
	fs::write(&image, &image_bytes).unwrap();

	success(invoke("verify", &image, &["--quick"]));
	assert!(!invoke("verify", &image, &["--container"]).status.success());
	assert_eq!(
		report("stat", &image, "/nested/three", &[])["kind"],
		"regular_file"
	);
	assert_eq!(success(invoke("cat", &image, &["/one"])), HARD_LINK_DATA);
	assert_eq!(
		success(invoke("cat", &image, &["/four"])),
		SECOND_INODE_DATA
	);
	assert_eq!(
		success(invoke("cat", &image, &["/one", "--fork", "resource"])),
		HARD_LINK_RESOURCE,
	);
	assert_eq!(fs::read(&image).unwrap(), image_bytes);
}

fn decimal(number: &Value) -> u64 {
	number.as_str().unwrap().parse().unwrap()
}
