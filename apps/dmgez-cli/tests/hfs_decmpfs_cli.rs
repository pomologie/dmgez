#![cfg(feature = "hfsplus-read")]

use std::{
	env,
	fs::{self, File, OpenOptions},
	io::{Read, Seek, SeekFrom, Write},
	path::{Path, PathBuf},
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";
const NATIVE_CAPTURE: &str = "native-decmpfs-zlib.bin";
const NATIVE_TAIL: &str = "native-decmpfs-tail.bin";
const NATIVE_LINK_OVERLAY: &str = "native-decmpfs-hardlinks.bin";
const NATIVE_VOLUME_BYTES: u64 = 64 * 1024 * 1024;
const NATIVE_RESOURCE_OFFSET: u64 = 12_066_816;
const NATIVE_RESOURCE_BYTES: usize = 1115;
const DECODED_BLOCK_BYTES: usize = 64 * 1024;
const INLINE_INPUT: &[u8] = b"portable-zlib-inline-fixture\n";
const INLINE_REPETITIONS: usize = 1024;
const RESOURCE_INPUT_BLOCKS: [u8; 3] = *b"ABC";
const RESOURCE_TAIL_BYTES: usize = 513;
const DATA_FORK: &str = "data";
const RESOURCE_FORK: &str = "resource";
const INLINE_PATH: &str = "/inline";
const RESOURCE_PATH: &str = "/resource";
const INLINE_LINK_PATH: &str = "/inline-hardlink";
const RESOURCE_LINK_PATH: &str = "/resource-hardlink";
const DECODER_UNAVAILABLE: &str = "decmpfs zlib support is disabled";
const DECODER_CAPABILITY: &str = "hfsplus-decmpfs";

#[test]
fn native_compressed_data_decodes_only_with_the_opt_in_feature() {
	let capabilities = successful_bytes(Command::new(BINARY).arg("capabilities").output().unwrap());
	let state = if cfg!(feature = "hfsplus-decmpfs") {
		"compiled"
	} else {
		"not_compiled"
	};
	let capabilities = String::from_utf8(capabilities).unwrap();
	let expected = format!("{DECODER_CAPABILITY} = {state}");
	assert!(
		capabilities.lines().any(|line| line == expected),
		"{capabilities}"
	);

	let scratch = tempfile::tempdir().unwrap();
	let image = scratch.path().join("native-hfsplus.raw");
	materialize(&image, None);

	assert_logical_data(
		&image,
		INLINE_PATH,
		&INLINE_INPUT.repeat(INLINE_REPETITIONS),
	);
	assert_logical_data(&image, RESOURCE_PATH, &native_logical_resource());
}

#[test]
fn native_stored_resource_fork_remains_exact_under_both_decoder_profiles() {
	let scratch = tempfile::tempdir().unwrap();
	let image = scratch.path().join("native-hfsplus.raw");
	materialize(&image, None);

	let mut native = File::open(&image).unwrap();
	let mut expected = [0; NATIVE_RESOURCE_BYTES];
	native
		.seek(SeekFrom::Start(NATIVE_RESOURCE_OFFSET))
		.unwrap();
	native.read_exact(&mut expected).unwrap();

	assert_eq!(
		successful_bytes(cat(&image, RESOURCE_PATH, RESOURCE_FORK)),
		expected
	);
	assert!(successful_bytes(cat(&image, INLINE_PATH, RESOURCE_FORK)).is_empty());
}

#[test]
fn compressed_hard_link_aliases_read_the_shared_inode_forks() {
	let scratch = tempfile::tempdir().unwrap();
	let image = scratch.path().join("native-hfsplus-hardlinks.raw");
	materialize(&image, Some(NATIVE_LINK_OVERLAY));

	let original_resource = successful_bytes(cat(&image, RESOURCE_PATH, RESOURCE_FORK));
	let alias_resource = successful_bytes(cat(&image, RESOURCE_LINK_PATH, RESOURCE_FORK));
	assert_eq!(original_resource.len(), NATIVE_RESOURCE_BYTES);
	assert_eq!(alias_resource, original_resource);
	assert_logical_data(
		&image,
		INLINE_LINK_PATH,
		&INLINE_INPUT.repeat(INLINE_REPETITIONS),
	);
	assert_logical_data(&image, RESOURCE_LINK_PATH, &native_logical_resource());
}

#[cfg(feature = "hfsplus-decmpfs")]
#[test]
fn corruption_in_a_later_native_resource_frame_stops_logical_output() {
	// The fixture ledger records the zlib frame for the third logical block.
	const FRAME_INDEX: usize = 2;
	const FRAME_OFFSET: u64 = 466;
	const FRAME_BYTES: u64 = 85;
	const FRAME_FINAL_BYTE: u64 = NATIVE_RESOURCE_OFFSET + FRAME_OFFSET + FRAME_BYTES - 1;

	let scratch = tempfile::tempdir().unwrap();
	let image = scratch.path().join("corrupt-hfsplus.raw");
	materialize(&image, None);
	let mut copy = OpenOptions::new()
		.read(true)
		.write(true)
		.open(&image)
		.unwrap();
	let mut byte = [0];
	copy.seek(SeekFrom::Start(FRAME_FINAL_BYTE)).unwrap();
	copy.read_exact(&mut byte).unwrap();
	copy.seek(SeekFrom::Start(FRAME_FINAL_BYTE)).unwrap();
	copy.write_all(&[byte[0] ^ 1]).unwrap();
	drop(copy);

	let output = cat(&image, RESOURCE_PATH, DATA_FORK);
	let expected = native_logical_resource();
	assert!(!output.status.success(), "{output:?}");
	assert!(output.stdout.len() < expected.len());
	assert!(expected.starts_with(&output.stdout));
	let error = String::from_utf8_lossy(&output.stderr);
	assert!(
		error.contains(&format!("decmpfs block {FRAME_INDEX}")),
		"{error}"
	);
}

#[cfg(all(feature = "build", feature = "hfsplus-decmpfs"))]
#[test]
fn logical_data_decodes_through_both_the_udif_and_hfs_compression_layers() {
	const FORMAT: &str = "udzo";
	const RUN_LIMIT: &str = "128";
	const ZLIB_ENCODING: &str = "zlib";

	let scratch = tempfile::tempdir().unwrap();
	let raw = scratch.path().join("native-hfsplus.raw");
	let image = scratch.path().join("native-hfsplus.dmg");
	materialize(&raw, None);
	successful_bytes(
		Command::new(BINARY)
			.arg("wrap-raw")
			.arg(&raw)
			.args(["--format", FORMAT, "-o"])
			.arg(&image)
			.output()
			.unwrap(),
	);

	let json = successful_bytes(
		Command::new(BINARY)
			.arg("inspect")
			.arg(&image)
			.args(["--json", "--run-limit", RUN_LIMIT])
			.output()
			.unwrap(),
	);
	let report: serde_json::Value = serde_json::from_slice(&json).unwrap();
	let runs = report["run_page"]["runs"].as_array().unwrap();
	assert!(runs.iter().any(|run| {
		let logical = &run["logical"];
		let start = logical["offset_bytes"]
			.as_str()
			.unwrap()
			.parse::<u64>()
			.unwrap();
		let length = logical["length_bytes"]
			.as_str()
			.unwrap()
			.parse::<u64>()
			.unwrap();
		run["encoding"]["kind"] == ZLIB_ENCODING
			&& (start..start + length).contains(&NATIVE_RESOURCE_OFFSET)
	}));

	let output = Command::new(BINARY)
		.arg("cat")
		.arg(&image)
		.args([RESOURCE_PATH, "--fork", DATA_FORK])
		.output()
		.unwrap();
	assert_eq!(successful_bytes(output), native_logical_resource());
}

fn assert_logical_data(image: &Path, path: &str, expected: &[u8]) {
	let output = cat(image, path, DATA_FORK);
	if cfg!(feature = "hfsplus-decmpfs") {
		assert_eq!(successful_bytes(output), expected);
	} else {
		assert!(!output.status.success(), "{output:?}");
		assert!(output.stdout.is_empty(), "{output:?}");
		let error = String::from_utf8_lossy(&output.stderr);
		assert!(error.contains(DECODER_UNAVAILABLE), "{error}");
	}
}

fn cat(image: &Path, path: &str, fork: &str) -> Output {
	Command::new(BINARY)
		.arg("cat")
		.arg(image)
		.args([path, "--raw", "--fork", fork])
		.output()
		.unwrap()
}

fn successful_bytes(output: Output) -> Vec<u8> {
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	output.stdout
}

fn native_logical_resource() -> Vec<u8> {
	let tail = hfsplus_fixture(NATIVE_TAIL);
	assert_eq!(tail.len(), RESOURCE_TAIL_BYTES);
	let mut expected =
		Vec::with_capacity(RESOURCE_INPUT_BLOCKS.len() * DECODED_BLOCK_BYTES + tail.len());
	for byte in RESOURCE_INPUT_BLOCKS {
		expected.resize(expected.len() + DECODED_BLOCK_BYTES, byte);
	}
	expected.extend_from_slice(&tail);
	expected
}

fn materialize(path: &Path, overlay: Option<&str>) {
	let mut image = OpenOptions::new()
		.write(true)
		.create_new(true)
		.open(path)
		.unwrap();
	image.set_len(NATIVE_VOLUME_BYTES).unwrap();
	write_capture(&mut image, &hfsplus_fixture(NATIVE_CAPTURE));
	if let Some(overlay) = overlay {
		write_capture(&mut image, &hfsplus_fixture(overlay));
	}
}

// The sibling ledger describes the capture as big-endian lengths and offsets
// followed by the original spans. Re-create a sparse raw file without writing
// the uncaptured bytes.
fn write_capture(image: &mut File, mut capture: &[u8]) {
	let length = u64::from_be_bytes(field(&mut capture));
	assert_eq!(length, NATIVE_VOLUME_BYTES);
	let spans = u16::from_be_bytes(field(&mut capture));
	for _ in 0..spans {
		let offset = u64::from_be_bytes(field(&mut capture));
		let count = u32::from_be_bytes(field(&mut capture));
		assert!(
			offset
				.checked_add(u64::from(count))
				.is_some_and(|end| end <= length)
		);
		let (bytes, remaining) = capture.split_at(count as usize);
		image.seek(SeekFrom::Start(offset)).unwrap();
		image.write_all(bytes).unwrap();
		capture = remaining;
	}
	assert!(capture.is_empty());
}

fn field<const WIDTH: usize>(capture: &mut &[u8]) -> [u8; WIDTH] {
	let (value, remaining) = capture.split_first_chunk().unwrap();
	*capture = remaining;
	*value
}

fn hfsplus_fixture(name: &str) -> Vec<u8> {
	let root = PathBuf::from(
		env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies HFS+ fixtures"),
	);
	let file = root.join(name);
	fs::read(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()))
}
