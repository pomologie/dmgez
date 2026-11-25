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
		"--identifier <ID>",
	] {
		assert!(help.contains(element), "{help}");
	}
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
