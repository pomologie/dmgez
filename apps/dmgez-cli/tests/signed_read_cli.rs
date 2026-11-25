#![cfg(feature = "inspect")]

use std::{fs, path::Path, process::Command};

use serde_json::Value;

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const SIGNED: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const RAW: &[u8] = include_bytes!("../../../tests/fixtures/expected/tiny.raw");
// Physical geometry recorded by the native fixture's provenance manifest.
const SIGNATURE_OFFSET: usize = 3084;
const SIGNATURE_BYTES: usize = 351;

fn json(command: &str, path: &Path, arguments: &[&str], code: i32) -> Value {
	let output = Command::new(BINARY)
		.arg(command)
		.arg(path)
		.args(arguments)
		.arg("--json")
		.output()
		.unwrap();
	assert_eq!(output.status.code(), Some(code), "{output:?}");
	serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn inspects_verifies_and_exports_the_native_signed_image() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("signed.dmg");
	let raw = directory.path().join("export.raw");
	fs::write(&image, SIGNED).unwrap();
	let report = json("inspect", &image, &[], 0);
	let signature = &report["container"]["embedded_signature"];
	assert_eq!(signature["offset_bytes"], SIGNATURE_OFFSET.to_string());
	assert_eq!(signature["length_bytes"], SIGNATURE_BYTES.to_string());
	assert_eq!(report["coverage"]["security"], "not_checked");
	for (mode, payload) in [("--quick", "not_checked"), ("--container", "passed")] {
		let report = json("verify", &image, &[mode], 0);
		assert_eq!(&report["container"]["embedded_signature"], signature);
		assert_eq!(report["coverage"]["structure"], "passed");
		assert_eq!(report["coverage"]["payload"], payload);
		assert_eq!(report["coverage"]["security"], "not_checked");
	}
	let output = Command::new(BINARY)
		.arg("unwrap-raw")
		.arg(&image)
		.arg("-o")
		.arg(&raw)
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	assert_eq!(fs::read(raw).unwrap(), RAW);
	assert_eq!(fs::read(image).unwrap(), SIGNED);
}

#[test]
fn malformed_signature_framing_fails_structure_and_prevents_raw_output() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("malformed.dmg");
	let raw = directory.path().join("unpublished.raw");
	let mut malformed = SIGNED.to_vec();
	malformed[SIGNATURE_OFFSET] ^= 1;
	fs::write(&image, &malformed).unwrap();
	let report = json("verify", &image, &["--quick"], 1);
	assert_eq!(report["coverage"]["structure"], "failed");
	assert_eq!(report["coverage"]["security"], "not_checked");
	assert_eq!(report["diagnostic"]["stage"], "structure");
	let output = Command::new(BINARY)
		.arg("unwrap-raw")
		.arg(&image)
		.arg("-o")
		.arg(&raw)
		.output()
		.unwrap();
	assert!(!output.status.success(), "{output:?}");
	assert!(!raw.exists());
	assert_eq!(fs::read(image).unwrap(), malformed);
}

#[cfg(all(
	feature = "signing-rust",
	any(target_os = "linux", target_os = "macos", windows)
))]
#[test]
fn container_crcs_do_not_verify_the_signature_digest() {
	// The unpaged SHA-256 digest ends the CodeDirectory immediately before Requirements.
	const REQUIREMENTS_OFFSET: usize = 3415;
	const SHA256_BYTES: usize = 32;
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join("invalid-digest.dmg");
	let mut tampered = SIGNED.to_vec();
	tampered[REQUIREMENTS_OFFSET - SHA256_BYTES] ^= 1;
	fs::write(&image, tampered).unwrap();
	let container = json("verify", &image, &["--container"], 0);
	assert_eq!(container["coverage"]["payload"], "passed");
	assert_eq!(container["coverage"]["security"], "not_checked");
	let signature = Command::new(BINARY)
		.args(["sign", "verify"])
		.arg(&image)
		.args(["--backend", "rust", "--json"])
		.output()
		.unwrap();
	assert_eq!(signature.status.code(), Some(1), "{signature:?}");
	let report: Value = serde_json::from_slice(&signature.stdout).unwrap();
	assert_eq!(report["status"], "failed");
}
