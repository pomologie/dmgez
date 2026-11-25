use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const EXIT_ERROR: i32 = 2;
const SOURCE: &str = "Installer.dmg";
const DESTINATION: &str = "Signed.dmg";
const IDENTIFIER: &str = "org.dmgez.unsigned-installer-fixture";
const UNUSED_FINGERPRINT: &str = "0000000000000000000000000000000000000000";
const AD_HOC: &str = "--ad-hoc";

fn sign_dmg(source: &Path, destination: &Path, identity: &str) -> Command {
	let mut command = Command::new(BINARY);
	command
		.args(["sign", "dmg"])
		.arg(source)
		.arg("--output")
		.arg(destination)
		.arg(format!("--identity={identity}"))
		.args(["--identifier", IDENTIFIER, "--json"]);
	command
}

fn assert_error(output: &Output, expected: &str) {
	assert_eq!(output.status.code(), Some(EXIT_ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

#[test]
fn documents_required_disk_signing_arguments() {
	let output = Command::new(BINARY)
		.args(["sign", "dmg", "--help"])
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	let help = String::from_utf8_lossy(&output.stdout);
	for element in [
		"<SOURCE>",
		"--output <PATH>",
		"--identity <SHA1>",
		AD_HOC,
		"--identifier <ID>",
	] {
		assert!(help.contains(element), "{help}");
	}
}

#[test]
fn refuses_both_signing_modes_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_dmg(&source, &destination, UNUSED_FINGERPRINT)
		.arg(AD_HOC)
		.output()
		.unwrap();
	assert_error(&output, "cannot be used with");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

fn sign_ad_hoc(source: &Path, destination: &Path) -> Command {
	let mut command = Command::new(BINARY);
	command
		.args(["sign", "dmg"])
		.arg(source)
		.arg("--output")
		.arg(destination)
		.args([AD_HOC, "--identifier", IDENTIFIER, "--json"]);
	command
}

#[cfg(not(feature = "signing-rust"))]
#[test]
fn refuses_disabled_ad_hoc_signing_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_ad_hoc(&source, &destination).output().unwrap();
	assert_error(&output, "feature not compiled: signing-rust");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(
	feature = "signing-rust",
	any(target_os = "linux", target_os = "macos", windows)
))]
#[test]
fn auto_publishes_an_ad_hoc_dmg_using_apple_codesign() {
	use std::fs;

	const UNSIGNED: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO.dmg");
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	fs::write(&source, UNSIGNED).unwrap();
	let output = sign_ad_hoc(&source, &destination).output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["backend"], "rust", "{report}");
	assert_eq!(report["mode"], "ad_hoc", "{report}");
	assert_eq!(report["identifier"], IDENTIFIER, "{report}");
	assert!(report["identity"].is_null(), "{report}");
	assert_eq!(report["assessment"], "not_performed", "{report}");
	assert_ne!(
		report["unsigned_sha256"], report["signed_sha256"],
		"{report}"
	);
	assert_eq!(fs::read(source).unwrap(), UNSIGNED);
	let backends: &[&str] = if cfg!(all(feature = "signing-native", target_os = "macos")) {
		&["rust"]
	} else {
		&["auto", "rust"]
	};
	for backend in backends {
		let verification = Command::new(BINARY)
			.args(["sign", "verify"])
			.arg(&destination)
			.args(["--backend", backend, "--json"])
			.output()
			.unwrap();
		assert!(verification.status.success(), "{verification:?}");
		let verified: serde_json::Value = serde_json::from_slice(&verification.stdout).unwrap();
		assert_eq!(verified["backend"], "rust", "{verified}");
		assert_eq!(verified["status"], "verified", "{verified}");
		assert_eq!(verified["assessment"], "not_performed", "{verified}");
	}

	#[cfg(target_os = "macos")]
	{
		let native = Command::new("/usr/bin/codesign")
			.args(["--verify", "--strict"])
			.arg(&destination)
			.output()
			.unwrap();
		assert!(native.status.success(), "{native:?}");
	}
}

#[cfg(feature = "signing-rust")]
#[test]
fn refuses_explicit_native_ad_hoc_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_ad_hoc(&source, &destination)
		.args(["--backend", "native"])
		.output()
		.unwrap();
	assert_error(&output, "unavailable");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(not(feature = "signing-native"))]
#[test]
fn refuses_disabled_disk_signing_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_dmg(&source, &destination, UNUSED_FINGERPRINT)
		.output()
		.unwrap();
	assert_error(&output, "feature not compiled: signing-native");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(feature = "signing-native")]
#[test]
fn refuses_unavailable_disk_signing_backends_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let backends: &[&str] = if cfg!(target_os = "macos") {
		&["rust"]
	} else {
		&["auto", "native", "rust"]
	};
	for backend in backends {
		let output = sign_dmg(&source, &destination, UNUSED_FINGERPRINT)
			.args(["--backend", backend])
			.output()
			.unwrap();
		assert_error(&output, "unavailable");
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(feature = "signing-native", target_os = "macos"))]
#[test]
fn refuses_ad_hoc_identity_before_accessing_paths() {
	const AD_HOC_IDENTITY: &str = "-";
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_dmg(&source, &destination, AD_HOC_IDENTITY)
		.args(["--backend", "native"])
		.output()
		.unwrap();
	assert_error(&output, "fingerprint");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}
