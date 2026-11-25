use std::{fmt::Write as _, fs, io::ErrorKind, path::Path, process::Output};

use sha2::{Digest, Sha256};

use super::{BINARY, MANIFEST_NAME};

const OUTPUT_NAME: &str = "release.dmg";
const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Release Fixture"
output = "release.dmg"
format = "udro"
[[contents]]
type = "directory"
destination = "/Data"
[release]
verify = true
"#;

fn run(directory: &Path, additional: &[&str]) -> Output {
	std::process::Command::new(BINARY)
		.current_dir(directory)
		.args(["build", MANIFEST_NAME, "--release", "--json"])
		.args(additional)
		.output()
		.unwrap()
}

#[test]
fn release_dry_run_uses_the_build_plan_report_without_publishing() {
	let directory = tempfile::tempdir().unwrap();
	fs::write(directory.path().join(MANIFEST_NAME), PROJECT).unwrap();
	let output = run(directory.path(), &["--dry-run"]);
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr),
	);
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["operation"], "build");
	assert_eq!(report["mode"], "dry_run");
	assert_eq!(report["release"], "requested");
	assert!(report.get("steps").is_none());
	assert!(!directory.path().join(OUTPUT_NAME).exists());
}

#[test]
fn publishes_a_verified_unsigned_release_and_binds_the_report_to_its_bytes() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join(MANIFEST_NAME);
	let image_path = directory.path().join(OUTPUT_NAME);
	fs::write(&manifest, PROJECT).unwrap();
	let output = run(directory.path(), &[]);
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	let image = fs::read(&image_path).unwrap();
	let mut digest = String::new();
	for byte in Sha256::digest(&image) {
		write!(digest, "{byte:02x}").unwrap();
	}
	assert_eq!(report["artifact"]["sha256"], digest);
	assert_eq!(report["artifact"]["bytes"], image.len() as u64);
	let step = |name| {
		report["steps"]
			.as_array()
			.unwrap()
			.iter()
			.find(|step| step["stage"] == name)
			.unwrap()
	};
	assert_eq!(step("verify_image")["status"], "passed");
	assert_eq!(step("sign_image")["status"], "skipped");
	assert_eq!(step("publish")["status"], "passed");
	assert_eq!(fs::read_to_string(&manifest).unwrap(), PROJECT);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);

	let retry = run(directory.path(), &[]);
	assert!(!retry.status.success());
	assert_eq!(fs::read(image_path).unwrap(), image);
}

#[test]
fn rejects_a_requested_unavailable_backend_and_an_implicit_overwrite() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join(MANIFEST_NAME);
	let image = directory.path().join(OUTPUT_NAME);
	let signing = "[signing]\nenabled = true\napps = 'preserve'\ndmg = true\nbackend = 'rust'\n";
	fs::write(&manifest, format!("{PROJECT}\n{signing}")).unwrap();
	assert!(!run(directory.path(), &[]).status.success());
	assert_eq!(
		fs::metadata(&image).unwrap_err().kind(),
		ErrorKind::NotFound
	);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

	fs::write(&manifest, PROJECT).unwrap();
	assert!(
		!run(directory.path(), &["--set", "build.overwrite=true"])
			.status
			.success()
	);
	assert_eq!(fs::metadata(image).unwrap_err().kind(), ErrorKind::NotFound);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(all(
	target_os = "macos",
	feature = "signing-native",
	feature = "hfsplus-install"
))]
#[test]
fn explicitly_verifies_empty_application_inventory_and_reports_bound_digest() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join(MANIFEST_NAME);
	fs::write(manifest, format!("{PROJECT}\n[signing]\nenabled = true\n")).unwrap();
	let output = run(directory.path(), &[]);
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["verified_applications"]["backend"], "native");
	assert_eq!(report["verified_applications"]["count"], 0);
	assert_eq!(report["verified_applications"]["input"], report["unsigned"]);
	assert_eq!(report["unsigned"], report["artifact"]);
	assert!(directory.path().join(OUTPUT_NAME).is_file());
}

#[cfg(all(
	target_os = "macos",
	feature = "signing-native",
	feature = "hfsplus-install"
))]
#[test]
fn rejects_an_unsigned_application_before_building_or_publishing() {
	use std::os::unix::fs::PermissionsExt;

	const APP_PROJECT: &str = r#"
schema_version = 1
[image]
name = "Release Fixture"
output = "release.dmg"
format = "udro"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_extended_attributes = false
preserve_finder_info = false
[[contents]]
source = "Unsigned.app"
destination = "/Unsigned.app"
[signing]
enabled = true
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let app = root.join("Unsigned.app");
	let executable = app.join("Contents/MacOS/Witness");
	fs::create_dir_all(executable.parent().unwrap()).unwrap();
	fs::write(&executable, b"#!/bin/false\n").unwrap();
	fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
	fs::write(
		app.join("Contents/Info.plist"),
		br#"<plist><dict><key>CFBundleExecutable</key><string>Witness</string></dict></plist>"#,
	)
	.unwrap();
	fs::write(root.join(MANIFEST_NAME), APP_PROJECT).unwrap();
	let output = run(root, &[]);
	assert!(!output.status.success(), "{output:?}");
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["steps"][0]["stage"], "applications");
	assert_eq!(report["steps"][0]["status"], "failed");
	assert!(report["unsigned"].is_null());
	assert!(report["verified_applications"].is_null());
	assert!(report["published_path"].is_null());
	assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}
