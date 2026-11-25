use std::{fs, path::Path, process::Command};

use super::{
	extract, failure,
	native::{APM_HFS_SLOT, APM_IMAGE},
	success,
};

const XATTR: &str = "/usr/bin/xattr";
const QUARANTINE_KEY: &str = "com.apple.quarantine";
const QUARANTINE: &str = "0081;00000000;dmgez-extract-test;";

fn mark(path: &Path) {
	success(
		Command::new(XATTR)
			.args(["-w", QUARANTINE_KEY, QUARANTINE])
			.arg(path)
			.output()
			.unwrap(),
	);
}

fn assert_marker_retained(path: &Path) {
	let value = success(
		Command::new(XATTR)
			.args(["-p", QUARANTINE_KEY])
			.arg(path)
			.output()
			.unwrap(),
	);
	assert_eq!(String::from_utf8(value).unwrap().trim_end(), QUARANTINE);
}

#[test]
fn refuses_a_quarantined_outer_dmg_without_exposing_any_output() {
	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = root.join("installer.dmg");
	let output = root.join("installer");
	fs::write(&image, APM_IMAGE).unwrap();
	success(extract(
		&image,
		"/",
		&output,
		&["--partition", APM_HFS_SLOT, "--data-only", "--dry-run"],
	));

	mark(&image);
	for args in [
		&["--partition", APM_HFS_SLOT, "--data-only"][..],
		&["--partition", APM_HFS_SLOT, "--data-only", "--dry-run"][..],
	] {
		let diagnostic = failure(extract(&image, "/", &output, args));
		assert!(
			diagnostic.to_ascii_lowercase().contains("quarantine"),
			"{diagnostic}"
		);
		assert!(!output.exists());
	}
	assert_eq!(fs::read(&image).unwrap(), APM_IMAGE);
	assert_marker_retained(&image);
}

#[cfg(feature = "project-build")]
#[test]
fn refuses_a_quarantined_hfs_file_as_a_fork_or_tree_member() {
	const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Extraction Quarantine Witness"
output = "inner.dmg"
format = "udro"
size = "1MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = true
[[contents]]
source = "marked.bin"
destination = "/Tree/Marked.bin"
[[contents]]
source = "plain.bin"
destination = "/Plain.bin"
"#;
	const PAYLOAD: &[u8] = b"data bytes in a marked HFS+ regular file";
	const CONTROL: &[u8] = b"unrelated unmarked data bytes";

	let temporary = tempfile::tempdir().unwrap();
	let root = temporary.path().canonicalize().unwrap();
	let image = root.join("inner.dmg");
	let output = root.join("new");
	let marked = root.join("marked.bin");
	let plain = root.join("plain.bin");
	let project = root.join("inner.toml");
	fs::write(&marked, PAYLOAD).unwrap();
	mark(&marked);
	fs::write(&plain, CONTROL).unwrap();
	fs::write(&project, SETTINGS).unwrap();
	success(
		Command::new(super::BINARY)
			.arg("build")
			.arg(&project)
			.output()
			.unwrap(),
	);
	let source = fs::read(&image).unwrap();

	for (path, args) in [
		("/Tree/Marked.bin", &["--fork", "data"][..]),
		("/Tree/Marked.bin", &["--fork", "resource", "--dry-run"][..]),
		("/Tree", &["--data-only"][..]),
	] {
		let diagnostic = failure(extract(&image, path, &output, args));
		assert!(
			diagnostic.to_ascii_lowercase().contains("quarantine"),
			"{diagnostic}"
		);
		assert!(!output.exists());
	}

	success(extract(&image, "/Plain.bin", &output, &["--fork", "data"]));
	assert_eq!(fs::read(&output).unwrap(), CONTROL);
	assert_eq!(fs::read(&image).unwrap(), source);
	assert_marker_retained(&marked);
}
