use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const EXIT_ERROR: i32 = 2;
const SOURCE: &str = "Unsigned.app";
const DESTINATION: &str = "Signed.app";
const UNUSED_FINGERPRINT: &str = "0000000000000000000000000000000000000000";

fn sign_app(source: &Path, destination: &Path, identity: &str) -> Command {
	let mut command = Command::new(BINARY);
	command
		.args(["sign", "app"])
		.arg(source)
		.arg("--output")
		.arg(destination)
		.arg(format!("--identity={identity}"))
		.arg("--json");
	command
}

fn assert_error(output: &Output, expected: &str) {
	assert_eq!(output.status.code(), Some(EXIT_ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

#[test]
fn documents_application_signing_arguments() {
	let output = Command::new(BINARY)
		.args(["sign", "app", "--help"])
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	let help = String::from_utf8_lossy(&output.stdout);
	for element in [
		"<SOURCE>",
		"--output <PATH>",
		"--identity <SHA1>",
		"--entitlements <CODE_PATH=PLIST>",
	] {
		assert!(help.contains(element), "{help}");
	}
}

#[cfg(not(feature = "signing-native"))]
#[test]
fn refuses_disabled_app_signing_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_app(&source, &destination, UNUSED_FINGERPRINT)
		.output()
		.unwrap();
	assert_error(&output, "feature not compiled: signing-native");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(feature = "signing-native")]
#[test]
fn refuses_unavailable_app_signing_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let backends: &[&str] = if cfg!(target_os = "macos") {
		&["rust"]
	} else {
		&["auto", "native", "rust"]
	};
	for backend in backends {
		let output = sign_app(&source, &destination, UNUSED_FINGERPRINT)
			.args(["--backend", backend])
			.output()
			.unwrap();
		assert_error(&output, "unavailable");
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(feature = "signing-native")]
#[test]
fn refuses_malformed_entitlement_mappings_without_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	for mapping in ["Contents/Helpers/Tool", "=Rule.plist", ".="] {
		let output = sign_app(&source, &destination, UNUSED_FINGERPRINT)
			.args(["--entitlements", mapping])
			.output()
			.unwrap();
		assert_error(&output, "--entitlements requires CODE_PATH=PLIST");
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(feature = "signing-native", target_os = "macos"))]
#[test]
fn accepts_repeated_entitlement_mappings_and_refuses_an_ad_hoc_identity() {
	const AD_HOC_IDENTITY: &str = "-";
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let destination = directory.path().join(DESTINATION);
	let output = sign_app(&source, &destination, AD_HOC_IDENTITY)
		.args([
			"--backend",
			"native",
			"--entitlements",
			".=Main=Rules.plist",
			"--entitlements",
			"Contents/Helpers/Héłper=Nested Rules.plist",
		])
		.output()
		.unwrap();
	assert_error(&output, "fingerprint");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}
