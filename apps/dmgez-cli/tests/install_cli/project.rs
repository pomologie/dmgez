use std::{
	fs,
	os::unix::fs::{PermissionsExt, symlink},
	path::{Path, PathBuf},
	process::{Command, Output},
};

use serde_json::Value;

#[cfg(all(feature = "signing-native", target_os = "macos"))]
use super::POLICY_OPTIONS;
use super::{APP, BINARY, BUNDLE, install};

const PROJECT: &str = "dmg.toml";
const IMAGE: &str = "application.dmg";
const EXECUTABLE: &str = "Contents/MacOS/Witness";
const LINK: &str = "Contents/Entrypoint";
const LINK_TARGET: &str = "MacOS/Witness";
const PROGRAM: &[u8] = b"#!/bin/false\n";
const RESOURCE: &[u8] = b"application resource fork";
#[cfg(target_os = "macos")]
const NATIVE_FORK: &str = "..namedfork/rsrc";
const EXECUTE_MODE: u32 = 0o751;
const MODE_BITS: u32 = 0o7777;
const PLIST: &str = r#"<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Witness</string>
</dict></plist>"#;
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Install Witness"
output = "application.dmg"
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

#[test]
fn plans_and_publishes_one_verified_bundle_without_overwrite() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = build(&root, LINK_TARGET);
	let original = fs::read(&image).unwrap();
	let destination = root.join("destination");
	fs::create_dir(&destination).unwrap();
	let planned = report(
		install(&image, &destination, &["--dry-run", "--json"]),
		"planned",
		true,
	);
	assert_eq!(planned["published"], false);
	assert_eq!(planned["bundle_name"], BUNDLE);
	assert_eq!(
		planned["destination_directory"],
		destination.to_str().unwrap()
	);
	assert_eq!(
		planned["plan"]["payload_bytes"],
		PLIST.len() + PROGRAM.len() + LINK_TARGET.len() + resource_bytes()
	);
	assert_empty(&destination);

	let copied = report(install(&image, &destination, &["--json"]), "complete", true);
	assert_eq!(copied["published"], true);
	assert!(copied["signature_check"].is_null());
	assert!(copied["gatekeeper_assessment"].is_null());
	assert_eq!(copied["plan"], planned["plan"]);
	assert!(copied["residual_stage_name"].is_null());
	let bundle = destination.join(BUNDLE);
	let binary = bundle.join(EXECUTABLE);
	assert_eq!(fs::read(&binary).unwrap(), PROGRAM);
	assert_eq!(
		fs::metadata(&binary).unwrap().permissions().mode() & MODE_BITS,
		EXECUTE_MODE
	);
	assert_eq!(
		fs::read_link(bundle.join(LINK)).unwrap(),
		Path::new(LINK_TARGET)
	);
	assert_eq!(
		fs::symlink_metadata(bundle.join(LINK))
			.unwrap()
			.permissions()
			.mode() & MODE_BITS,
		fs::symlink_metadata(root.join(BUNDLE).join(LINK))
			.unwrap()
			.permissions()
			.mode() & MODE_BITS,
	);
	#[cfg(target_os = "macos")]
	assert_eq!(fs::read(binary.join(NATIVE_FORK)).unwrap(), RESOURCE);
	assert_eq!(
		fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap(),
		PLIST
	);
	assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);

	let conflict = report(install(&image, &destination, &["--json"]), "failed", false);
	assert_eq!(conflict["published"], false);
	assert!(conflict["error"].as_str().unwrap().contains("exists"));
	assert_eq!(fs::read(&binary).unwrap(), PROGRAM);
	assert_eq!(fs::read(&image).unwrap(), original);
}

#[cfg(all(feature = "signing-native", target_os = "macos"))]
#[test]
fn dry_run_reports_requested_checks_and_signature_refusal_prevents_publication() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = build(&root, LINK_TARGET);
	let destination = root.join("destination");
	fs::create_dir(&destination).unwrap();
	let [signature_option, assessment_option] = POLICY_OPTIONS;
	let planned = report(
		install(
			&image,
			&destination,
			&[signature_option, assessment_option, "--dry-run", "--json"],
		),
		"planned",
		true,
	);
	assert_eq!(planned["signature_check"], "pending");
	assert_eq!(planned["gatekeeper_assessment"], "pending");
	assert_empty(&destination);
	let refused = report(
		install(&image, &destination, &[signature_option, "--json"]),
		"failed",
		false,
	);
	assert_eq!(refused["signature_check"], "failed");
	assert!(refused["gatekeeper_assessment"].is_null());
	assert_eq!(refused["published"], false);
	assert!(!destination.join(BUNDLE).exists());
	let residual = refused["residual_stage_name"].as_str().unwrap();
	assert_eq!(
		fs::read(destination.join(residual).join(BUNDLE).join(EXECUTABLE)).unwrap(),
		PROGRAM
	);
}

#[test]
fn refuses_unconfined_links_and_an_exhausted_budget_before_staging() {
	for (link, arguments, expected) in [
		("../../Outside", &["--json"][..], "link target"),
		(
			LINK_TARGET,
			&["--max-bytes", "0", "--json"][..],
			"payload byte budget",
		),
	] {
		let temporary = tempfile::tempdir().unwrap();
		let root = temporary.path().canonicalize().unwrap();
		let image = build(&root, link);
		let destination = root.join("destination");
		fs::create_dir(&destination).unwrap();
		let output = install(&image, &destination, arguments);
		assert!(
			String::from_utf8_lossy(&output.stderr).contains(expected),
			"{output:?}"
		);
		let refusal = report(output, "failed", false);
		assert_eq!(refusal["published"], false);
		assert!(refusal["residual_stage_name"].is_null());
		assert_empty(&destination);
	}
}

fn build(root: &Path, link: &str) -> PathBuf {
	let app = root.join(BUNDLE);
	let binary = app.join(EXECUTABLE);
	fs::create_dir_all(binary.parent().unwrap()).unwrap();
	fs::write(&binary, PROGRAM).unwrap();
	fs::set_permissions(&binary, fs::Permissions::from_mode(EXECUTE_MODE)).unwrap();
	#[cfg(target_os = "macos")]
	fs::write(binary.join(NATIVE_FORK), RESOURCE).unwrap();
	fs::write(app.join("Contents/Info.plist"), PLIST).unwrap();
	symlink(link, app.join(LINK)).unwrap();
	let project = root.join(PROJECT);
	fs::write(&project, SETTINGS).unwrap();
	let result = Command::new(BINARY)
		.arg("build")
		.arg(project)
		.output()
		.unwrap();
	assert!(result.status.success(), "{result:?}");
	root.join(IMAGE)
}

fn resource_bytes() -> usize {
	if cfg!(target_os = "macos") {
		RESOURCE.len()
	} else {
		0
	}
}

fn report(output: Output, status: &str, success: bool) -> Value {
	assert_eq!(output.status.success(), success, "{output:?}");
	let report: Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["operation"], "install");
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["status"], status);
	assert_eq!(report["source_path"], APP);
	report
}

fn assert_empty(directory: &Path) {
	assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
}
