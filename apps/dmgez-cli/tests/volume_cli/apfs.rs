//! D-READ-03 APFS browsing over native blocks retained by the sibling engine.

use std::{
	collections::BTreeSet,
	env, fs,
	io::{Seek, SeekFrom, Write},
	path::{Path, PathBuf},
	process::Output,
};

use serde_json::{Value, json};

use super::invoke;

const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_APFS_FIXTURES";
const HARD_LINK_FIXTURE: &str = "sonoma-hard-link-blocks.bin";
const PAYLOAD_FIXTURE: &str = "sonoma-payload-blocks.bin";
const SPARSE_FIXTURE: &str = "sonoma-sparse-blocks.bin";
const NATIVE_BLOCK_BYTES: usize = 4096;
const NATIVE_VOLUME_BYTES: u64 = 64 << 20;
const PACKED_ADDRESS_BYTES: usize = size_of::<u64>();
const PACKED_RECORD_BYTES: usize = PACKED_ADDRESS_BYTES + NATIVE_BLOCK_BYTES;
const RAW_APFS: &[&str] = &["--raw", "--filesystem", "apfs"];

// Each fixture's sibling JSON ledger records data from the native oracle.
const VOLUME_NAME: &str = "Pomologie APFS Hard Links";
const VOLUME_UUID: &str = "D37600DC-3EF5-4393-8E07-31DBAA91E9B6";
const VOLUME_OID: &str = "1026";
const CHECKPOINT_XID: &str = "4";
const UNRETAINED_XID: &str = "999999";
const ABSENT_VOLUME_OID: &str = "999999";
const HARD_LINK_INODE: &str = "17";
const HARD_LINK_COUNT: u64 = 3;
const HARD_LINK_MODE: u64 = 0o100644;
const HARD_LINK_DATA: &[u8] = b"APFS shared data fork\n";
const HARD_LINK_DATA_REPETITIONS: usize = 250;
const HARD_LINK_RESOURCE: &[u8] = b"APFS shared resource fork\n";
const HARD_LINK_RESOURCE_REPETITIONS: usize = 200;
const HARD_LINK_PATHS: [&str; 3] = ["/primary.bin", "/Docs/alias.bin", "/More/Écho.bin"];
const PAYLOAD_BYTES: usize = 19_000;
const PAYLOAD_FACTOR: usize = 17;
const PAYLOAD_MODULUS: usize = 251;
const PAYLOAD_LINK_TARGET: &[u8] = b"../large.bin";
const SPARSE_BYTES: usize = 786_469;
const ALL_HOLE_BYTES: usize = 12_317;
const SPARSE_WRITES: [(usize, usize, usize, usize); 2] =
	[(131_072, 17, 23, 251), (524_288, 29, 67, 253)];
const SPARSE_RESOURCE: &[u8] = b"APFS sparse-file resource fork\n";
const SPARSE_RESOURCE_REPETITIONS: usize = 174;

#[test]
fn discovers_and_selects_native_volume_by_uuid_object_id_and_checkpoint() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("native-apfs.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);

	let discovery = json_report("volumes", &image, &["--raw", "--xid", CHECKPOINT_XID]);
	assert_eq!(discovery["volume"]["kind"], "whole_disk");
	assert_eq!(discovery["checkpoint_xid"], CHECKPOINT_XID);
	let [volume] = entries(&discovery, "volumes") else {
		panic!("expected one native volume: {discovery}");
	};
	assert_eq!(volume["name"], VOLUME_NAME);
	assert_eq!(volume["object_id"], VOLUME_OID);
	assert_uuid(&volume["uuid"]);

	let implicit = browse_report("stat", &image, "/", &[]);
	assert_eq!(implicit["volume"], discovery["volume"]);
	assert_eq!(implicit["kind"], "directory");
	let selection = &implicit["apfs_volume"];
	assert_eq!(selection["object_id"], VOLUME_OID);
	assert_eq!(selection["checkpoint_xid"], CHECKPOINT_XID);
	assert_uuid(&selection["uuid"]);
	for selector in [VOLUME_UUID, VOLUME_OID] {
		assert_eq!(
			browse_report(
				"stat",
				&image,
				"/",
				&["--volume", selector, "--xid", CHECKPOINT_XID],
			),
			implicit,
		);
	}
	for (flag, invalid) in [
		("--volume", ABSENT_VOLUME_OID),
		("--volume", "malformed-uuid"),
		("--xid", UNRETAINED_XID),
		("--xid", "0"),
	] {
		failure(call("stat", &image, "/", &[flag, invalid, "--json"]));
	}
	failure(invoke(
		"stat",
		&image,
		&["/", "--raw", "--volume", VOLUME_OID],
	));
	failure(invoke(
		"stat",
		&image,
		&["/", "--raw", "--xid", CHECKPOINT_XID],
	));
	failure(invoke("stat", &image, &["/", "--filesystem", "apfs"]));
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn refuses_a_partitionless_container_with_a_bad_checksum_or_truncated_extent() {
	let directory = tempfile::tempdir().unwrap();
	let corrupt = directory.path().join("corrupt.raw");
	let truncated = directory.path().join("truncated.raw");
	for image in [&corrupt, &truncated] {
		materialize(image, HARD_LINK_FIXTURE, &[]);
	}
	let mut checksum = fs::OpenOptions::new()
		.read(true)
		.write(true)
		.open(&corrupt)
		.unwrap();
	let mut first = [0];
	std::io::Read::read_exact(&mut checksum, &mut first).unwrap();
	checksum.rewind().unwrap();
	checksum.write_all(&[!first[0]]).unwrap();
	drop(checksum);
	fs::OpenOptions::new()
		.write(true)
		.open(&truncated)
		.unwrap()
		.set_len(NATIVE_VOLUME_BYTES - NATIVE_BLOCK_BYTES as u64)
		.unwrap();
	for image in [&corrupt, &truncated] {
		failure(invoke("volumes", image, &["--raw", "--json"]));
		failure(call("stat", image, "/", &["--json"]));
	}
}

#[test]
fn pages_native_entries_and_reads_both_forks_through_each_hard_link() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("hardlinks.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);

	let first = browse_report("ls", &image, "/", &["--limit", "1"]);
	assert_eq!(first["next_offset"], 1);
	assert_eq!(names(&first), ["Docs"]);
	let last = browse_report("ls", &image, "/", &["--offset", "1", "--limit", "2"]);
	assert!(last["next_offset"].is_null());
	assert_eq!(names(&last), ["More", "primary.bin"]);

	let shallow = browse_report("tree", &image, "/", &["--depth", "1"]);
	assert_eq!(names(&shallow), ["Docs", "More", "primary.bin"]);
	let deep = browse_report("tree", &image, "/", &["--depth", "2"]);
	assert!(names(&deep).contains(&"alias.bin"));
	assert!(names(&deep).contains(&"Écho.bin"));
	let capped = browse_report("tree", &image, "/", &["--limit", "1"]);
	assert_eq!(names(&capped), ["Docs"]);
	assert_eq!(capped["truncated"], true);

	let data = HARD_LINK_DATA.repeat(HARD_LINK_DATA_REPETITIONS);
	let resource = HARD_LINK_RESOURCE.repeat(HARD_LINK_RESOURCE_REPETITIONS);
	for path in HARD_LINK_PATHS {
		let stat = browse_report("stat", &image, path, &[]);
		assert_eq!(stat["kind"], "regular_file");
		assert_eq!(stat["mode"], HARD_LINK_MODE);
		assert_eq!(stat["hard_link_count"], HARD_LINK_COUNT);
		assert_eq!(stat["hard_link_storage_id"], HARD_LINK_INODE);
		assert_eq!(stat["native_id"], HARD_LINK_INODE);
		assert_eq!(success(call("cat", &image, path, &[])), data);
		assert_eq!(
			success(call("cat", &image, path, &["--fork", "resource"])),
			resource,
		);
	}
}

#[test]
fn reports_links_without_following_them_and_streams_empty_and_large_files() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("payloads.raw");
	materialize(&image, PAYLOAD_FIXTURE, &[]);

	let link = browse_report("stat", &image, "/link", &[]);
	assert_eq!(link["kind"], "symlink");
	assert_eq!(link["link_target"], json!(PAYLOAD_LINK_TARGET));
	for (command, path) in [
		("cat", "/link"),
		("ls", "/link"),
		("stat", "/link/large.bin"),
	] {
		failure(call(command, &image, path, &[]));
	}
	assert!(success(call("cat", &image, "/empty.txt", &[])).is_empty());
	let data: Vec<_> = (0..PAYLOAD_BYTES)
		.map(|index| {
			u8::try_from((index * PAYLOAD_FACTOR + index / NATIVE_BLOCK_BYTES) % PAYLOAD_MODULUS)
				.unwrap()
		})
		.collect();
	assert_eq!(success(call("cat", &image, "/large.bin", &[])), data);
}

#[test]
fn streams_native_sparse_files_and_separate_resource_fork() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("sparse.raw");
	materialize(&image, SPARSE_FIXTURE, &[]);

	let mut data = vec![0; SPARSE_BYTES];
	for (offset, factor, addend, modulus) in SPARSE_WRITES {
		for (index, byte) in data[offset..][..NATIVE_BLOCK_BYTES].iter_mut().enumerate() {
			*byte = u8::try_from((index * factor + addend) % modulus).unwrap();
		}
	}
	assert_eq!(success(call("cat", &image, "/sparse.bin", &[])), data);
	assert_eq!(
		success(call("cat", &image, "/all-hole.bin", &[])),
		vec![0; ALL_HOLE_BYTES],
	);
	assert_eq!(
		success(call("cat", &image, "/sparse.bin", &["--fork", "resource"])),
		SPARSE_RESOURCE.repeat(SPARSE_RESOURCE_REPETITIONS),
	);
}

#[cfg(feature = "build")]
#[test]
fn opens_native_apfs_from_a_bounded_compressed_udif_partition() {
	const MBR_TABLE_OFFSET: usize = 446;
	const MBR_TYPE_OFFSET: usize = MBR_TABLE_OFFSET + 4;
	const MBR_FIRST_LBA_OFFSET: usize = MBR_TABLE_OFFSET + 8;
	const MBR_LENGTH_LBA_OFFSET: usize = MBR_TABLE_OFFSET + 12;
	const MBR_SIGNATURE_OFFSET: usize = 510;
	const MBR_SIGNATURE: [u8; 2] = [0x55, 0xaa];
	const NON_DOS_DATA_PARTITION: u8 = 0xda;
	const SECTOR_BYTES: usize = 512;
	const FIRST_LBA: u32 = 2048;
	const PARTITION_SLOT: &str = "1";
	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("mbr.raw");
	let image = directory.path().join("apfs-partition.dmg");
	let start = FIRST_LBA as usize * SECTOR_BYTES;
	let mut prefix = vec![0; start];
	prefix[MBR_TYPE_OFFSET] = NON_DOS_DATA_PARTITION;
	prefix[MBR_FIRST_LBA_OFFSET..][..size_of::<u32>()].copy_from_slice(&FIRST_LBA.to_le_bytes());
	let sectors = u32::try_from(NATIVE_VOLUME_BYTES / SECTOR_BYTES as u64).unwrap();
	prefix[MBR_LENGTH_LBA_OFFSET..][..size_of::<u32>()].copy_from_slice(&sectors.to_le_bytes());
	prefix[MBR_SIGNATURE_OFFSET..SECTOR_BYTES].copy_from_slice(&MBR_SIGNATURE);
	materialize(&raw, HARD_LINK_FIXTURE, &prefix);
	success(invoke(
		"wrap-raw",
		&raw,
		&["--format", "udzo", "-o", image.to_str().unwrap()],
	));

	let discovery = json_report("volumes", &image, &["--partition", PARTITION_SLOT]);
	assert_eq!(
		discovery["volume"],
		json!({"kind": "partition", "id": {"scheme": "mbr", "slot": 1}})
	);
	assert_eq!(discovery["volumes"][0]["object_id"], VOLUME_OID);
	assert_eq!(
		success(invoke(
			"cat",
			&image,
			&[
				HARD_LINK_PATHS[0],
				"--filesystem",
				"apfs",
				"--partition",
				PARTITION_SLOT,
				"--fork",
				"resource",
			],
		)),
		HARD_LINK_RESOURCE.repeat(HARD_LINK_RESOURCE_REPETITIONS),
	);
	failure(invoke("volumes", &image, &["--partition", "2", "--json"]));
}

fn materialize(image: &Path, fixture: &str, prefix: &[u8]) {
	let directory = env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies APFS fixtures");
	let path = PathBuf::from(directory).join(fixture);
	let packed = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
	let (records, remaining) = packed.as_chunks::<PACKED_RECORD_BYTES>();
	assert!(remaining.is_empty());

	let mut file = fs::File::create_new(image).unwrap();
	file.set_len(prefix.len() as u64 + NATIVE_VOLUME_BYTES)
		.unwrap();
	file.write_all(prefix).unwrap();
	let mut blocks = BTreeSet::new();
	for record in records {
		let (address, data) = record.split_first_chunk().unwrap();
		let block = u64::from_le_bytes(*address);
		assert!(blocks.insert(block));
		assert!(block < NATIVE_VOLUME_BYTES / NATIVE_BLOCK_BYTES as u64);
		let offset = prefix.len() as u64 + block * NATIVE_BLOCK_BYTES as u64;
		file.seek(SeekFrom::Start(offset)).unwrap();
		file.write_all(data).unwrap();
	}
}

fn call(command: &str, image: &Path, path: &str, options: &[&str]) -> Output {
	let args: Vec<_> = [path]
		.into_iter()
		.chain(RAW_APFS.iter().copied())
		.chain(options.iter().copied())
		.collect();
	invoke(command, image, &args)
}

fn success(output: Output) -> Vec<u8> {
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(output.stderr.is_empty(), "{output:?}");
	output.stdout
}

fn failure(output: Output) {
	assert!(!output.status.success(), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	assert!(!output.stderr.is_empty(), "{output:?}");
}

fn json_report(command: &str, image: &Path, options: &[&str]) -> Value {
	let args: Vec<_> = ["--json"]
		.into_iter()
		.chain(options.iter().copied())
		.collect();
	let report: Value = serde_json::from_slice(&success(invoke(command, image, &args))).unwrap();
	assert_eq!(report["schema_version"], 1);
	report
}

fn browse_report(command: &str, image: &Path, path: &str, options: &[&str]) -> Value {
	let args: Vec<_> = ["--json"]
		.into_iter()
		.chain(options.iter().copied())
		.collect();
	let report: Value =
		serde_json::from_slice(&success(call(command, image, path, &args))).unwrap();
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["operation"], command);
	assert_eq!(report["path"], path);
	report
}

fn assert_uuid(value: &Value) {
	assert!(
		value.as_str().unwrap().eq_ignore_ascii_case(VOLUME_UUID),
		"{value}"
	);
}

fn entries<'a>(value: &'a Value, key: &str) -> &'a [Value] {
	value[key].as_array().unwrap()
}

fn names(value: &Value) -> Vec<&str> {
	entries(value, "entries")
		.iter()
		.map(|entry| {
			let name = &entry["name"];
			assert_eq!(name["encoding"], "utf8");
			name["value"].as_str().unwrap()
		})
		.collect()
}
