#![cfg(feature = "inspect")]

use std::{
	fs,
	path::Path,
	process::{Command, Output},
};

use serde_json::Value;

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
use std::{env, io::ErrorKind, path::PathBuf};

const NATIVE_APM: &[u8] =
	include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
// Apple's UDTO export records the range in native-maps.json next to the fixture.
const APM_HFS_OFFSET: u64 = 32_768;
const APM_HFS_BYTES: u64 = 16_744_448;
const APM_HFS_SLOT: u32 = 2;

#[test]
fn full_verification_reports_native_apm_scope_or_unavailable_backend() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("native-apm.dmg");
	fs::write(&image, NATIVE_APM).unwrap();
	let report = verify(&image, "--full", false);
	assert_eq!(report["outcome"], "partial");
	assert_eq!(report["coverage"]["structure"], "passed");
	assert_eq!(report["coverage"]["payload"], "passed");
	assert_eq!(report["coverage"]["security"], "not_checked");
	assert_eq!(
		report["capabilities"]["filesystem"],
		cfg!(feature = "hfsplus-verify")
	);

	let text = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("verify")
		.arg(&image)
		.arg("--full")
		.output()
		.unwrap();
	assert_status(&text, false);
	let text = String::from_utf8(text.stdout).unwrap();
	assert_eq!(
		text.lines()
			.any(|line| line == "filesystem_scope = reachable_structure"),
		cfg!(feature = "hfsplus-verify")
	);

	if cfg!(feature = "hfsplus-verify") {
		let check = assert_reachable(&report);
		let window = &check["window"];
		assert_eq!(window["id"]["kind"], "partition");
		assert_eq!(window["id"]["id"]["scheme"], "apm");
		assert_eq!(window["id"]["id"]["slot"], APM_HFS_SLOT);
		assert_eq!(
			window["logical"]["offset_bytes"],
			APM_HFS_OFFSET.to_string()
		);
		assert_eq!(window["logical"]["length_bytes"], APM_HFS_BYTES.to_string());
	} else {
		assert_eq!(report["coverage"]["filesystem"], "unavailable");
		assert_eq!(report["diagnostic"]["stage"], "filesystem");
		assert_eq!(report["diagnostic"]["category"], "feature_not_compiled");
		assert!(report.get("filesystem_check").is_none());
	}

	for mode in ["--quick", "--container"] {
		let report = verify(&image, mode, true);
		assert_eq!(report["outcome"], "passed");
		assert_eq!(report["coverage"]["filesystem"], "not_checked");
		assert!(report.get("filesystem_check").is_none());
	}
	assert_eq!(fs::read(&image).unwrap(), NATIVE_APM);
}

fn assert_reachable(report: &Value) -> &Value {
	assert_eq!(report["coverage"]["filesystem"], "partial", "{report:#}");
	let check = &report["filesystem_check"];
	assert_eq!(check["evidence"]["scope"], "reachable_structure");
	assert_eq!(check["all_filesystems_checked"], false);
	assert!(decimal(&check["charged_read_requests"]) > 0);
	assert!(decimal(&check["charged_read_bytes"]) > 0);
	check
}

fn decimal(value: &Value) -> u64 {
	value.as_str().unwrap().parse().unwrap()
}

fn verify(image: &Path, mode: &str, success: bool) -> Value {
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("verify")
		.arg(image)
		.args([mode, "--json"])
		.output()
		.unwrap();
	assert_status(&output, success);
	serde_json::from_slice(&output.stdout).unwrap()
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

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
const HFS_CATALOG_FIXTURE: &str = "native-catalog-volume.bin";
#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
const HFSX_FIXTURE: &str = "native-hfsx.bin";
#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
const CASE_FOLDED_PROVENANCE: &str = "native-hfsx-case-folded.json";

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
#[test]
fn checks_reachable_structure_on_native_hfs_plus_and_hfsx() {
	let directory = tempfile::tempdir().unwrap();
	let catalog = fixture(HFS_CATALOG_FIXTURE);
	let case_folded = materialize_case_folded_hfsx(&catalog);
	let fixtures = [
		("catalog", catalog),
		("case-sensitive", materialize_hfsx(&fixture(HFSX_FIXTURE))),
	];
	for (name, volume) in fixtures.into_iter().chain(case_folded) {
		let image = wrap(directory.path(), name, &volume);
		let report = verify(&image, "--full", false);
		assert_eq!(report["outcome"], "partial");
		assert_eq!(report["coverage"]["payload"], "passed");
		let window = &assert_reachable(&report)["window"];
		assert_eq!(window["id"]["kind"], "whole_disk");
		assert_eq!(decimal(&window["logical"]["offset_bytes"]), 0);
		assert_eq!(
			decimal(&window["logical"]["length_bytes"]),
			volume.len() as u64
		);
	}
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
#[test]
fn fresh_valid_container_does_not_mask_a_corrupt_hfs_catalog_count() {
	// HFS Plus Volume Header (Apple TN1150). The count is independent of the
	// signature, version and block geometry the disk probe uses for admission.
	const PRIMARY_HEADER_OFFSET: usize = 1024;
	const FILE_COUNT_OFFSET: usize = 32;
	const DECLARED_COUNT: usize = PRIMARY_HEADER_OFFSET + FILE_COUNT_OFFSET;
	let directory = tempfile::tempdir().unwrap();
	let mut bytes = fixture(HFS_CATALOG_FIXTURE);
	let count = &mut bytes[DECLARED_COUNT..][..size_of::<u32>()];
	let original = u32::from_be_bytes(count.try_into().unwrap());
	count.copy_from_slice(&original.checked_add(1).unwrap().to_be_bytes());
	let image = wrap(directory.path(), "bad-count", &bytes);
	let report = verify(&image, "--full", false);
	assert_eq!(report["outcome"], "failed");
	assert_eq!(report["coverage"]["structure"], "passed");
	assert_eq!(report["coverage"]["payload"], "passed");
	assert_eq!(report["coverage"]["filesystem"], "failed");
	assert_eq!(report["diagnostic"]["stage"], "filesystem");
	assert_eq!(report["diagnostic"]["category"], "corrupt");
	let check = &report["filesystem_check"];
	assert_eq!(check["all_filesystems_checked"], false);
	assert!(check.get("evidence").is_none());
	assert!(decimal(&check["charged_read_requests"]) > 0);
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
fn fixture(name: &str) -> Vec<u8> {
	let path = fixture_root().join(name);
	fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
fn fixture_root() -> PathBuf {
	const FIXTURES_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";
	PathBuf::from(env::var_os(FIXTURES_ENV).expect("compose supplies HFS+ fixtures"))
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
fn materialize_case_folded_hfsx(base: &[u8]) -> Option<(&'static str, Vec<u8>)> {
	let path = fixture_root().join(CASE_FOLDED_PROVENANCE);
	let document = match fs::read(&path) {
		Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap(),
		Err(error) if error.kind() == ErrorKind::NotFound => {
			eprintln!(
				"Case-folded HFSX witness skipped: {} unavailable",
				path.display()
			);
			return None;
		}
		Err(error) => panic!("{}: {error}", path.display()),
	};
	assert_eq!(document["base_fixture"], HFS_CATALOG_FIXTURE);
	let overlays = document["derivation"]["byte_overlays"].as_array().unwrap();
	assert!(!overlays.is_empty());
	let mut bytes = base.to_vec();
	for overlay in overlays {
		let offset = usize::try_from(overlay["offset"].as_u64().unwrap()).unwrap();
		let value = u8::try_from(overlay["value"].as_u64().unwrap()).unwrap();
		assert_ne!(bytes[offset], value);
		bytes[offset] = value;
	}
	Some(("case-insensitive", bytes))
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
fn materialize_hfsx(mut source: &[u8]) -> Vec<u8> {
	// native-hfsx-case-sensitive.json describes this fixed witness format.
	const DISK_BYTES: usize = 24 * 1024 * 1024;
	const RETAINED_RUNS: u16 = 19;
	fn next<const WIDTH: usize>(source: &mut &[u8]) -> [u8; WIDTH] {
		let (value, rest) = source.split_first_chunk().unwrap();
		*source = rest;
		*value
	}
	assert_eq!(u64::from_be_bytes(next(&mut source)), DISK_BYTES as u64);
	assert_eq!(u16::from_be_bytes(next(&mut source)), RETAINED_RUNS);
	let mut bytes = vec![0; DISK_BYTES];
	for _ in 0..RETAINED_RUNS {
		let offset = usize::try_from(u64::from_be_bytes(next(&mut source))).unwrap();
		let length = usize::try_from(u32::from_be_bytes(next(&mut source))).unwrap();
		let (run, rest) = source.split_at(length);
		bytes[offset..][..length].copy_from_slice(run);
		source = rest;
	}
	assert!(source.is_empty());
	bytes
}

#[cfg(all(feature = "build", feature = "hfsplus-verify"))]
fn wrap(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
	let input = root.join(format!("{name}.raw"));
	let image = root.join(format!("{name}.dmg"));
	fs::write(&input, bytes).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("wrap-raw")
		.arg(input)
		.args(["--format", "udzo", "-o"])
		.arg(&image)
		.output()
		.unwrap();
	assert_status(&output, true);
	image
}
