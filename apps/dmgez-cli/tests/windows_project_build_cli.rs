#![cfg(all(windows, feature = "project-build"))]

use std::{fs, process::Command};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const MANIFEST: &str = "dmg.toml";
const PAYLOAD_NAME: &str = "release nôtes.txt";
const PAYLOAD: &[u8] = b"synthetic Windows NTFS project payload";
const OUTPUT: &str = "installer.dmg";
const STRICT_METADATA: &str = "build.fail_on_metadata_loss=true";
const SETTINGS: &str = r#"
schema_version = 1

[image]
name = "Windows Installer"
output = "installer.dmg"
format = "udro"

[[contents]]
source = "release nôtes.txt"
destination = "/Release Notes.txt"

[metadata]
policy = "report"
preserve_permissions = false
preserve_ownership = false
preserve_timestamps = false
preserve_resource_forks = false
preserve_extended_attributes = false
preserve_finder_info = true
"#;

#[test]
fn builds_from_an_ntfs_source_and_reports_unavailable_metadata() {
	let temp = tempfile::tempdir().unwrap();
	let source = temp.path().join(PAYLOAD_NAME);
	let output = temp.path().join(OUTPUT);
	fs::write(&source, PAYLOAD).unwrap();
	fs::write(temp.path().join(MANIFEST), SETTINGS).unwrap();
	let build = |options: &[&str]| {
		Command::new(BINARY)
			.current_dir(temp.path())
			.args(["build", MANIFEST])
			.args(options)
			.output()
			.unwrap()
	};

	let preview = build(&["--dry-run"]);
	assert!(
		preview.status.success(),
		"{}",
		String::from_utf8_lossy(&preview.stderr)
	);
	assert!(String::from_utf8_lossy(&preview.stdout).contains("mode=dry-run"));
	assert!(String::from_utf8_lossy(&preview.stderr).contains("FinderInfo (SourceUnavailable)"));
	assert!(!output.exists());

	let strict = build(&["--set", STRICT_METADATA]);
	assert!(!strict.status.success());
	let error = String::from_utf8_lossy(&strict.stderr);
	assert!(
		error.contains("FinderInfo") && error.contains("SourceUnavailable"),
		"{error}"
	);
	assert!(!output.exists());

	let overwrite = build(&["--overwrite"]);
	assert!(!overwrite.status.success());
	assert!(String::from_utf8_lossy(&overwrite.stderr).contains("atomic path replacement"));
	assert!(!output.exists());

	let result = build(&[]);
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	let image = fs::read(&output).unwrap();
	assert!(image.windows(PAYLOAD.len()).any(|window| window == PAYLOAD));
	fs::write(&source, b"changed synthetic payload").unwrap();
	assert!(!build(&[]).status.success());
	assert_eq!(fs::read(&output).unwrap(), image);
}
