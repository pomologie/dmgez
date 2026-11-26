use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use super::*;

const PROJECT: &str = "dmg.toml";
const BUNDLE: &str = "Witness.app";
const EXECUTABLE: &str = "Contents/MacOS/Witness";
const PLIST: &str = "Contents/Info.plist";
const EXECUTE_MODE: u32 = 0o755;
const DATA_ONLY_MODE: u32 = 0o644;
const FIRST_PAYLOAD_MUTATION: usize = 4096;
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Installer Test Witness"
output = "installer.dmg"
format = "udro"
size = "2MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_extended_attributes = false
preserve_finder_info = false
[[contents]]
source = "Witness.app"
destination = "/Witness.app"
"#;
const SHORTCUT: &str = r#"
[[contents]]
type = "symlink"
destination = "/Applications"
target = "/Applications"
"#;

fn build(root: &Path, executable_mode: u32, shortcut: &str) {
	let bundle = root.join(BUNDLE);
	let executable = bundle.join(EXECUTABLE);
	fs::create_dir_all(executable.parent().unwrap()).unwrap();
	fs::write(&executable, b"#!/bin/false\n").unwrap();
	fs::set_permissions(executable, fs::Permissions::from_mode(executable_mode)).unwrap();
	fs::write(
		bundle.join(PLIST),
		br#"<plist><dict><key>CFBundleExecutable</key><string>Witness</string></dict></plist>"#,
	)
	.unwrap();
	fs::write(root.join(PROJECT), format!("{SETTINGS}{shortcut}")).unwrap();
	let output = Command::new(BINARY)
		.current_dir(root)
		.args(["build", PROJECT])
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
}

#[test]
fn tests_finished_app_and_shortcut_without_moving_or_writing_the_image() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, EXECUTE_MODE, SHORTCUT);
	let image = root.join(IMAGE_NAME);
	let bytes = fs::read(&image).unwrap();
	let result = report(installer().current_dir(root).output().unwrap(), 0);
	assert_eq!(result["source"]["status"], "passed");
	assert_eq!(result["container"]["status"], "passed");
	assert_eq!(result["container"]["evidence"]["master_verified"], true);
	assert_eq!(result["layout"]["status"], "passed");
	assert_eq!(result["layout"]["evidence"]["applications"], 1);
	for gate in ["signature", "ticket", "native_assessment"] {
		assert_eq!(result[gate]["status"], "skipped");
	}
	assert_eq!(fs::read(image).unwrap(), bytes);
	assert_eq!(fs::read_dir(root).unwrap().count(), 3);
}

#[test]
fn distinguishes_layout_findings_from_container_corruption() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, DATA_ONLY_MODE, "");
	let result = report(installer().current_dir(root).output().unwrap(), 1);
	assert_eq!(result["container"]["status"], "passed");
	assert_eq!(result["layout"]["status"], "failed");
	assert!(
		result["layout"]["evidence"]["diagnostics"]
			.as_array()
			.unwrap()
			.len()
			>= 2
	);
	let image = root.join(IMAGE_NAME);
	let mut bytes = fs::read(&image).unwrap();
	bytes[FIRST_PAYLOAD_MUTATION] ^= 1;
	fs::write(&image, &bytes).unwrap();
	let result = report(
		installer()
			.current_dir(root)
			.arg("--require-signature")
			.output()
			.unwrap(),
		1,
	);
	assert_eq!(result["container"]["status"], "failed");
	assert_eq!(result["layout"]["status"], "blocked");
	assert_eq!(result["signature"]["status"], "blocked");
	assert_eq!(fs::read(image).unwrap(), bytes);
}

#[cfg(not(all(feature = "signing-native", target_os = "macos")))]
#[test]
fn unavailable_native_policy_yields_exit_two_alongside_passing_layout() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, EXECUTE_MODE, SHORTCUT);
	let result = report(
		installer()
			.current_dir(root)
			.arg("--require-native-assessment")
			.output()
			.unwrap(),
		2,
	);
	assert_eq!(result["layout"]["status"], "passed");
	assert_eq!(result["native_assessment"]["status"], "unavailable");
	assert_eq!(result["signature"]["status"], "skipped");
	assert_eq!(result["ticket"]["status"], "skipped");
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
#[test]
fn reports_unsigned_image_and_missing_ticket_separately() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, EXECUTE_MODE, SHORTCUT);
	let result = report(
		installer()
			.current_dir(root)
			.args(["--require-signature", "--require-ticket"])
			.output()
			.unwrap(),
		1,
	);
	assert_eq!(result["signature"]["status"], "failed");
	assert_eq!(result["signature"]["evidence"], "unsigned");
	assert_eq!(result["ticket"]["status"], "failed");
	assert_eq!(result["ticket"]["evidence"], "missing");
	assert_eq!(result["native_assessment"]["status"], "skipped");
}

#[cfg(all(feature = "signing-native", target_os = "macos"))]
#[test]
fn tests_a_natively_signed_image_and_reports_static_signature_verification() {
	const CODESIGN: &str = "/usr/bin/codesign";
	const AD_HOC_IDENTITY: &str = "-";
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, EXECUTE_MODE, SHORTCUT);
	let image = root.join(IMAGE_NAME);
	let signed = Command::new(CODESIGN)
		.args(["--sign", AD_HOC_IDENTITY])
		.arg(&image)
		.output()
		.unwrap();
	assert!(signed.status.success(), "{signed:?}");
	let bytes = fs::read(&image).unwrap();
	let result = report(
		installer()
			.current_dir(root)
			.arg("--require-signature")
			.output()
			.unwrap(),
		0,
	);
	assert_eq!(result["layout"]["status"], "passed");
	assert_eq!(result["signature"]["evidence"], "verified");
	assert_eq!(result["ticket"]["status"], "skipped");
	assert_eq!(result["native_assessment"]["status"], "skipped");
	assert_eq!(fs::read(image).unwrap(), bytes);
}
