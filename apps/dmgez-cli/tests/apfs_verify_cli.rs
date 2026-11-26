//! D-READ-04 checks against native APFS records and independently damaged layers.

#![cfg(feature = "inspect")]

use std::{
	fs,
	path::Path,
	process::{Command, Output},
};

use serde_json::Value;

#[cfg(feature = "apfs-read")]
#[path = "support/native_apfs.rs"]
mod fixture;

#[cfg(feature = "apfs-read")]
use fixture::{
	HARD_LINK_CHECKPOINT_XID, HARD_LINK_FIXTURE, HARD_LINK_VOLUME_OID, HARD_LINK_VOLUME_UUID,
	NATIVE_VOLUME_BYTES, SPARSE_FIXTURE, materialize,
};

#[cfg(feature = "apfs-read")]
#[path = "support/native_apfs_mutations.rs"]
mod native_mutations;

#[cfg(all(feature = "build", feature = "apfs-read"))]
#[path = "apfs_verify_cli/udzo.rs"]
mod udzo;

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const APFS_VERIFY: &[&str] = &["--full", "--filesystem", "apfs"];

#[cfg(all(
	feature = "installer-test",
	feature = "build",
	feature = "apfs-read",
	any(target_os = "linux", target_os = "macos")
))]
#[test]
fn checks_finished_installer_layout_on_a_selected_native_apfs_checkpoint() {
	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("native.raw");
	let image = directory.path().join("native.dmg");
	materialize(&raw, HARD_LINK_FIXTURE, &[]);
	let wrapped = Command::new(BINARY)
		.arg("wrap-raw")
		.arg(raw)
		.arg("--output")
		.arg(&image)
		.args(["--format", "udzo"])
		.output()
		.unwrap();
	assert!(wrapped.status.success(), "{wrapped:?}");
	let checked = Command::new(BINARY)
		.arg("test")
		.arg(&image)
		.args([
			"--profile",
			"installer",
			"--filesystem",
			"apfs",
			"--volume",
			HARD_LINK_VOLUME_OID,
			"--xid",
			HARD_LINK_CHECKPOINT_XID,
			"--json",
		])
		.output()
		.unwrap();
	assert_eq!(checked.status.code(), Some(1), "{checked:?}");
	let report: Value = serde_json::from_slice(&checked.stdout).unwrap();
	assert_eq!(report["filesystem"], "apfs");
	assert_eq!(report["apfs_volume"]["object_id"], HARD_LINK_VOLUME_OID);
	assert_eq!(
		report["apfs_volume"]["checkpoint_xid"],
		HARD_LINK_CHECKPOINT_XID
	);
	assert_eq!(report["container"]["status"], "passed");
	assert_eq!(report["layout"]["status"], "failed");
	assert_eq!(report["layout"]["evidence"]["applications"], 0);
	assert_eq!(
		report["layout"]["evidence"]["diagnostics"][0]["rule"],
		"missing_application"
	);
}

// The engine's native hard-link witness records two forks and four inline
// provenance attributes. Three hard-link names share one set of payloads.
#[cfg(feature = "apfs-read")]
const HARD_LINK_LOGICAL_BYTES: u64 = 5_500 + 5_200 + 4 * 11;
#[cfg(feature = "apfs-read")]
const UNRETAINED_XID: &str = "999999";

#[cfg(not(feature = "apfs-read"))]
#[test]
fn explicit_apfs_verification_reports_unavailable_after_container_checks() {
	const NATIVE_APM: &[u8] =
		include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("native-apm.dmg");
	fs::write(&image, NATIVE_APM).unwrap();

	let report = verify(&image, &[], false);
	assert_eq!(report["outcome"], "partial");
	assert_eq!(report["coverage"]["structure"], "passed");
	assert_eq!(report["coverage"]["payload"], "passed");
	assert_eq!(report["capabilities"]["filesystem"], false);
	assert_eq!(report["coverage"]["filesystem"], "unavailable");
	assert_eq!(report["diagnostic"]["stage"], "filesystem");
	assert_eq!(report["diagnostic"]["category"], "feature_not_compiled");
	assert!(report.get("filesystem_check").is_none());
	assert_eq!(fs::read(&image).unwrap(), NATIVE_APM);
}

#[cfg(feature = "apfs-read")]
#[test]
fn native_raw_selects_volume_by_uuid_or_oid_and_reads_each_shared_fork_once() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("hard-links.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	let metadata = fs::metadata(&image).unwrap();
	let budget = HARD_LINK_LOGICAL_BYTES.to_string();

	for selector in [HARD_LINK_VOLUME_UUID, HARD_LINK_VOLUME_OID] {
		let report = verify(
			&image,
			&[
				"--raw",
				"--volume",
				selector,
				"--xid",
				HARD_LINK_CHECKPOINT_XID,
				"--max-payload-bytes",
				&budget,
			],
			false,
		);
		assert_eq!(report["coverage"]["structure"], "passed");
		assert_eq!(report["coverage"]["payload"], "not_present");
		assert_eq!(report["capabilities"]["container_checksums"], false);
		assert!(report.get("container").is_none());
		assert!(report.get("checksums").is_none());
		let window = &assert_native_reachable_data(&report)["window"];
		assert_eq!(window["id"]["kind"], "whole_disk");
		assert_eq!(window["logical"]["offset_bytes"], "0");
		assert_eq!(
			window["logical"]["length_bytes"],
			NATIVE_VOLUME_BYTES.to_string()
		);
	}

	let missing_xid = verify(&image, &["--raw", "--xid", UNRETAINED_XID], false);
	assert_filesystem_failure(&missing_xid, "conflict", "failed");
	let missing_volume = verify(&image, &["--raw", "--volume", UNRETAINED_XID], false);
	assert_filesystem_failure(&missing_volume, "conflict", "failed");

	let text = invoke(&image, &["--raw"]);
	assert_status(&text, false);
	let text = String::from_utf8(text.stdout).unwrap();
	assert!(
		text.lines()
			.any(|line| line == "filesystem_scope = reachable_data"),
		"{text}"
	);
	let after = fs::metadata(&image).unwrap();
	assert_eq!(after.len(), metadata.len());
	assert_eq!(after.modified().unwrap(), metadata.modified().unwrap());
}

#[cfg(feature = "apfs-read")]
#[test]
fn raw_partition_map_requires_an_explicit_slot_and_preserves_window_identity() {
	use serde_json::json;

	const SECTOR_BYTES: usize = 512;
	const APFS_FIRST_LBA: u32 = 2048;
	const OTHER_FIRST_LBA: u32 = 1;
	const OTHER_SECTORS: u32 = 1;
	const APFS_SLOT: &str = "2";
	const MBR_TABLE_OFFSET: usize = 446;
	const MBR_ENTRY_BYTES: usize = 16;
	const MBR_TYPE_OFFSET: usize = 4;
	const MBR_FIRST_LBA_OFFSET: usize = 8;
	const MBR_LENGTH_LBA_OFFSET: usize = 12;
	const MBR_SIGNATURE_OFFSET: usize = 510;
	const MBR_SIGNATURE: [u8; 2] = [0x55, 0xaa];
	const NON_DOS_DATA_PARTITION: u8 = 0xda;
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("partitions.raw");
	let apfs_start = APFS_FIRST_LBA as usize * SECTOR_BYTES;
	let mut prefix = vec![0; apfs_start];
	let apfs_sectors = u32::try_from(NATIVE_VOLUME_BYTES / SECTOR_BYTES as u64).unwrap();
	for (index, (start, length)) in [
		(OTHER_FIRST_LBA, OTHER_SECTORS),
		(APFS_FIRST_LBA, apfs_sectors),
	]
	.into_iter()
	.enumerate()
	{
		let entry = MBR_TABLE_OFFSET + index * MBR_ENTRY_BYTES;
		prefix[entry + MBR_TYPE_OFFSET] = NON_DOS_DATA_PARTITION;
		prefix[entry + MBR_FIRST_LBA_OFFSET..][..size_of::<u32>()]
			.copy_from_slice(&start.to_le_bytes());
		prefix[entry + MBR_LENGTH_LBA_OFFSET..][..size_of::<u32>()]
			.copy_from_slice(&length.to_le_bytes());
	}
	prefix[MBR_SIGNATURE_OFFSET..SECTOR_BYTES].copy_from_slice(&MBR_SIGNATURE);
	materialize(&image, HARD_LINK_FIXTURE, &prefix);

	let unspecific = verify(&image, &["--raw"], false);
	assert_filesystem_failure(&unspecific, "conflict", "failed");
	assert_eq!(unspecific["diagnostic"]["detail"]["candidates"], 2);

	let selected = verify(&image, &["--raw", "--partition", APFS_SLOT], false);
	let window = &assert_native_reachable_data(&selected)["window"];
	assert_eq!(
		window["id"],
		json!({"kind": "partition", "id": {"scheme": "mbr", "slot": 2}})
	);
	assert_eq!(window["logical"]["offset_bytes"], apfs_start.to_string());
	assert_eq!(
		window["logical"]["length_bytes"],
		NATIVE_VOLUME_BYTES.to_string()
	);
}

#[cfg(feature = "apfs-read")]
#[test]
fn raw_payload_limit_charges_native_sparse_holes_without_claiming_completed_evidence() {
	// The sparse witness contains two files (one entirely holes), one resource
	// fork, and two inline provenance attributes.
	const SPARSE_LOGICAL_BYTES: u64 = 786_469 + 12_317 + 5_394 + 2 * 11;
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("sparse-payload-budget.raw");
	materialize(&image, SPARSE_FIXTURE, &[]);
	let exact = SPARSE_LOGICAL_BYTES.to_string();
	let report = verify(&image, &["--raw", "--max-payload-bytes", &exact], false);
	assert_eq!(report["outcome"], "partial");
	let evidence = &report["filesystem_check"]["evidence"];
	assert_eq!(evidence["scope"], "reachable_data");
	assert_eq!(evidence["logical_payload_bytes_read"], exact);

	let short = (SPARSE_LOGICAL_BYTES - 1).to_string();
	let report = verify(&image, &["--raw", "--max-payload-bytes", &short], false);
	assert_filesystem_failure(&report, "resource_limit", "unavailable");
	assert_eq!(report["outcome"], "partial");
	assert_eq!(report["coverage"]["payload"], "not_present");
}

#[cfg(feature = "apfs-read")]
#[test]
fn reports_unqualified_apfs_directory_names_as_unsupported() {
	use native_mutations::{edit_object, replace_field};

	const FSROOT_BLOCK: u64 = 206;
	const ROOT_DIRECTORY_NAME_OFFSET: usize = 731;
	const ORIGINAL_NAME: &[u8] = b"primary.bin";
	const UNICODE_11_NAME: &str = "\u{1c90}mary.bin";
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("unqualified-name.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	edit_object(&image, FSROOT_BLOCK, |block| {
		replace_field::<{ ORIGINAL_NAME.len() }>(
			block,
			ROOT_DIRECTORY_NAME_OFFSET,
			ORIGINAL_NAME.try_into().unwrap(),
			UNICODE_11_NAME.as_bytes().try_into().unwrap(),
		);
	});

	let report = verify(&image, &["--raw"], false);
	assert_filesystem_failure(&report, "unsupported_variant", "unsupported");
	assert_eq!(report["outcome"], "partial");
}

#[cfg(feature = "apfs-read")]
#[test]
fn newest_rejected_checkpoint_is_reported_with_earlier_native_evidence() {
	use native_mutations::{INCOMPLETE_CHECKPOINT_XID, advertise_incomplete_checkpoint};

	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("incomplete-checkpoint.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	advertise_incomplete_checkpoint(&image);

	let default = verify(&image, &["--raw"], false);
	let check = assert_native_reachable_data(&default);
	let warnings = &check["evidence"]["checkpoint_diagnostics"];
	assert_eq!(warnings["rejected_descriptors"], 1);
	let incomplete = INCOMPLETE_CHECKPOINT_XID.to_string();
	assert_eq!(warnings["advertised_xid"], incomplete);

	let exact = verify(&image, &["--raw", "--xid", &incomplete], false);
	assert_filesystem_failure(&exact, "corrupt", "failed");
	assert_eq!(exact["outcome"], "failed");
}

#[cfg(feature = "apfs-read")]
#[test]
fn bad_raw_bootstrap_checksum_fails_structure_before_volume_verification() {
	use std::io::{Read, Seek, Write};

	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("invalid-bootstrap.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	let mut file = fs::OpenOptions::new()
		.read(true)
		.write(true)
		.open(&image)
		.unwrap();
	let mut checksum = [0; size_of::<u64>()];
	file.read_exact(&mut checksum).unwrap();
	checksum[0] ^= 1;
	file.rewind().unwrap();
	file.write_all(&checksum).unwrap();
	drop(file);

	let report = verify(&image, &["--raw"], false);
	assert_eq!(report["outcome"], "failed");
	assert_eq!(report["coverage"]["structure"], "failed");
	assert_eq!(report["coverage"]["payload"], "not_present");
	assert_eq!(report["coverage"]["filesystem"], "not_checked");
	assert_eq!(report["diagnostic"]["stage"], "structure");
	assert_eq!(report["diagnostic"]["category"], "corrupt");
	assert!(report.get("filesystem_check").is_none());
}

#[cfg(feature = "apfs-read")]
fn assert_native_reachable_data(report: &Value) -> &Value {
	assert_eq!(report["outcome"], "partial", "{report:#}");
	assert_eq!(report["coverage"]["filesystem"], "partial", "{report:#}");
	assert_eq!(report["coverage"]["security"], "not_checked");
	assert!(report.get("diagnostic").is_none());
	let check = &report["filesystem_check"];
	assert_eq!(check["all_filesystems_checked"], false);
	let evidence = &check["evidence"];
	assert_eq!(evidence["scope"], "reachable_data");
	assert_eq!(evidence["volume_object_id"], HARD_LINK_VOLUME_OID);
	assert_eq!(
		evidence["volume_uuid"],
		HARD_LINK_VOLUME_UUID.to_lowercase()
	);
	assert_eq!(evidence["checkpoint_xid"], HARD_LINK_CHECKPOINT_XID);
	assert_eq!(
		evidence["logical_payload_bytes_read"],
		HARD_LINK_LOGICAL_BYTES.to_string()
	);
	assert!(
		check["charged_read_requests"]
			.as_str()
			.unwrap()
			.parse::<u64>()
			.unwrap() > 0
	);
	assert!(
		check["charged_read_bytes"]
			.as_str()
			.unwrap()
			.parse::<u64>()
			.unwrap() > 0
	);
	check
}

#[cfg(feature = "apfs-read")]
fn assert_filesystem_failure(report: &Value, category: &str, coverage: &str) {
	assert_eq!(report["diagnostic"]["stage"], "filesystem", "{report:#}");
	assert_eq!(report["diagnostic"]["category"], category, "{report:#}");
	assert_eq!(report["coverage"]["filesystem"], coverage);
	assert!(report["filesystem_check"].get("evidence").is_none());
}

fn verify(image: &Path, flags: &[&str], success: bool) -> Value {
	let args: Vec<_> = ["--json"]
		.into_iter()
		.chain(flags.iter().copied())
		.collect();
	let output = invoke(image, &args);
	assert_status(&output, success);
	let report: Value = serde_json::from_slice(&output.stdout)
		.unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["operation"], "verify");
	assert_eq!(report["mode"], "full");
	report
}

fn invoke(image: &Path, flags: &[&str]) -> Output {
	Command::new(BINARY)
		.arg("verify")
		.arg(image)
		.args(APFS_VERIFY)
		.args(flags)
		.output()
		.unwrap()
}

fn assert_status(output: &Output, success: bool) {
	assert_eq!(
		output.status.success(),
		success,
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}
