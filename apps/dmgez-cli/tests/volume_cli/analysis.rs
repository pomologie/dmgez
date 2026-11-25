use std::{fs, process::Output};

#[cfg(feature = "hfsplus-analysis")]
use serde_json::{Value, json};

use super::invoke;

const COMMAND: &str = "analyze";
const HFSPLUS: &str = "--hfsplus";
const LARGEST: &str = "--largest";
const PARTITION: &str = "--partition";
const JSON: &str = "--json";
const ABSENT_IMAGE: &str = "absent.dmg";
#[cfg(feature = "hfsplus-analysis")]
const FILESYSTEM: &str = "filesystem";

fn reject(output: Output, diagnostic: &str) {
	assert!(!output.status.success(), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	assert!(
		String::from_utf8_lossy(&output.stderr).contains(diagnostic),
		"{output:?}"
	);
}

#[cfg(not(feature = "hfsplus-analysis"))]
#[test]
fn analysis_without_the_hfs_profile_rejects_before_opening_input() {
	let directory = tempfile::tempdir().unwrap();
	reject(
		invoke(
			COMMAND,
			&directory.path().join(ABSENT_IMAGE),
			&[HFSPLUS, JSON],
		),
		"feature not compiled: hfsplus-analysis",
	);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn filesystem_analysis_arguments_require_explicit_hfs_scope() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join(ABSENT_IMAGE);
	for option in [PARTITION, LARGEST] {
		reject(invoke(COMMAND, &image, &[option, "1"]), HFSPLUS);
	}
}

#[cfg(feature = "hfsplus-analysis")]
#[test]
fn analyzes_a_native_apm_hfs_volume_without_changing_container_analysis() {
	const IMAGE: &str = "native-apm.dmg";
	const FIXTURE: &[u8] =
		include_bytes!("../../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	const APM_SLOT: u32 = 2;
	const APM_SLOT_ARG: &str = "2";
	const NON_HFS_SLOT: &str = "1";
	const BLOCK_BYTES: u64 = 4096;
	const TOTAL_BLOCKS: u64 = 4088;
	const FREE_BLOCKS: u64 = 3993;

	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join(IMAGE);
	fs::write(&image, FIXTURE).unwrap();

	let container = parse(invoke(COMMAND, &image, &[JSON]));
	assert!(container.get(FILESYSTEM).is_none());
	let mut report = parse(invoke(
		COMMAND,
		&image,
		&[HFSPLUS, PARTITION, APM_SLOT_ARG, JSON],
	));
	assert_eq!(
		report["volume"],
		json!({"kind": "partition", "id": {"scheme": "apm", "slot": APM_SLOT}}),
	);
	let allocation = &report[FILESYSTEM]["allocation"];
	assert_eq!(allocation["block_size"], BLOCK_BYTES);
	assert_eq!(allocation["total_blocks"], TOTAL_BLOCKS);
	assert_eq!(allocation["free_blocks"], FREE_BLOCKS);
	assert_eq!(
		allocation["metadata_blocks"].as_u64().unwrap()
			+ allocation["other_allocated_blocks"].as_u64().unwrap(),
		TOTAL_BLOCKS - FREE_BLOCKS,
	);
	assert_eq!(report[FILESYSTEM]["content"]["largest_files"], json!([]));
	assert_eq!(report[FILESYSTEM]["content"]["limits_reached"], json!([]));
	assert_eq!(report[FILESYSTEM]["content"]["complete"], true);
	report.as_object_mut().unwrap().remove(FILESYSTEM);
	report.as_object_mut().unwrap().remove("volume");
	assert_eq!(report, container);

	let wrong_slot = invoke(COMMAND, &image, &[HFSPLUS, PARTITION, NON_HFS_SLOT, JSON]);
	assert!(!wrong_slot.status.success(), "{wrong_slot:?}");
	assert!(wrong_slot.stdout.is_empty(), "{wrong_slot:?}");
	let usual_text = invoke(COMMAND, &image, &[]);
	let hfs_text = invoke(COMMAND, &image, &[HFSPLUS, PARTITION, APM_SLOT_ARG]);
	assert!(usual_text.status.success(), "{usual_text:?}");
	assert!(hfs_text.status.success(), "{hfs_text:?}");
	assert!(hfs_text.stdout.starts_with(&usual_text.stdout));
	assert_eq!(fs::read(&image).unwrap(), FIXTURE);
}

#[cfg(feature = "hfsplus-analysis")]
#[test]
fn rejects_largest_count_above_the_catalog_limit_before_opening_input() {
	use dmgez_hfsplus_read::analysis::MAX_LARGEST_FILES;

	let directory = tempfile::tempdir().unwrap();
	let too_many = (MAX_LARGEST_FILES + 1).to_string();
	reject(
		invoke(
			COMMAND,
			&directory.path().join(ABSENT_IMAGE),
			&[HFSPLUS, LARGEST, &too_many],
		),
		"--largest must be at most",
	);
}

#[cfg(all(feature = "build", feature = "hfsplus-analysis"))]
#[test]
fn ranks_the_nonempty_file_from_the_native_catalog_in_json_and_text() {
	use std::process::Command;

	use super::{BINARY, native_hfsplus::hfsplus_fixture};

	const NATIVE_CATALOG: &str = "native-catalog-volume.bin";
	const VOLUME_BYTES: usize = 86_016;
	const TOTAL_VISIBLE_FILES: u64 = 97;
	const NONEMPTY_PAYLOAD: &[u8] = b"catalog unicode payload\n";
	const TOP_PATH: [&str; 2] = ["Nested", "Re\u{301}sume\u{301}.txt"];

	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("catalog.raw");
	let image = directory.path().join("catalog.dmg");
	let bytes = hfsplus_fixture(NATIVE_CATALOG);
	assert_eq!(bytes.len(), VOLUME_BYTES);
	fs::write(&raw, &bytes).unwrap();
	let wrap = Command::new(BINARY)
		.args(["wrap-raw", "--format", "udro", "--output"])
		.arg(&image)
		.arg(&raw)
		.output()
		.unwrap();
	assert!(wrap.status.success(), "{wrap:?}");

	let report = parse(invoke(COMMAND, &image, &[HFSPLUS, LARGEST, "1", JSON]));
	assert_eq!(report["volume"], json!({"kind": "whole_disk"}));
	let content = &report[FILESYSTEM]["content"];
	assert_eq!(content["regular_files"], TOTAL_VISIBLE_FILES);
	let ranked = content["largest_files"].as_array().unwrap();
	assert_eq!(ranked.len(), 1);
	let path: Vec<Vec<u16>> = serde_json::from_value(ranked[0]["path"].clone()).unwrap();
	assert_eq!(
		path.iter()
			.map(|name| String::from_utf16(name).unwrap())
			.collect::<Vec<_>>(),
		TOP_PATH,
	);
	assert_eq!(
		ranked[0]["logical_bytes"],
		NONEMPTY_PAYLOAD.len().to_string()
	);
	let text = invoke(COMMAND, &image, &[HFSPLUS, LARGEST, "1"]);
	assert!(text.status.success(), "{text:?}");
	let text = String::from_utf8(text.stdout).unwrap();
	assert!(
		text.lines().any(|line| line.starts_with("largest_file = ")),
		"{text}"
	);
	assert_eq!(fs::read(&raw).unwrap(), bytes);
}

#[cfg(feature = "hfsplus-analysis")]
fn parse(output: Output) -> Value {
	assert!(output.status.success(), "{output:?}");
	serde_json::from_slice(&output.stdout).unwrap()
}
