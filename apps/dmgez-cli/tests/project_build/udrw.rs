use std::{fs, path::Path, process::Command};

use super::{BINARY, MANIFEST_NAME};

const IMAGE_BYTES: usize = 4 << 20;
const VOLUME_HEADER_OFFSET: usize = 1024;
const HFS_SIGNATURE: &[u8] = b"H+";
const RAW_IMAGE: &str = "raw-backed.dmg";
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Raw-backed fixture"
output = "raw-backed.dmg"
format = "udrw"
size = "4MiB"
[[contents]]
type = "directory"
destination = "/Data"
[[contents]]
type = "symlink"
destination = "/Applications"
target = "/Applications"
[build]
reproducible = true
source_date_epoch = 1700000000
"#;

fn cli(directory: &Path, arguments: &[&str]) -> String {
	let output = Command::new(BINARY)
		.current_dir(directory)
		.args(arguments)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	String::from_utf8(output.stdout).unwrap()
}

#[test]
fn builds_a_trailerless_udrw_image_with_exact_planned_geometry() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	fs::write(root.join(MANIFEST_NAME), SETTINGS).unwrap();
	let plan = cli(root, &["build", MANIFEST_NAME, "--dry-run"]);
	assert!(plan.lines().any(|line| line == "format=Udrw"));
	assert!(
		plan.lines()
			.any(|line| line == format!("physical_bytes={IMAGE_BYTES}"))
	);
	let path = root.join(RAW_IMAGE);
	assert!(!path.exists());
	cli(root, &["build", MANIFEST_NAME]);
	let bytes = fs::read(path).unwrap();
	assert_eq!(bytes.len(), IMAGE_BYTES);
	assert_eq!(
		&bytes[VOLUME_HEADER_OFFSET..][..HFS_SIGNATURE.len()],
		HFS_SIGNATURE,
	);
	let trailer = &bytes[bytes.len() - dmgez_udif::TRAILER_BYTES..];
	assert!(dmgez_udif::check_trailer_header(trailer.try_into().unwrap()).is_err());
	#[cfg(feature = "hfsplus-read")]
	{
		let listing = cli(root, &["ls", "--raw", RAW_IMAGE]);
		assert!(listing.contains("Applications"));
		assert!(listing.contains("Data"));
	}
}

#[cfg(feature = "inspect")]
#[test]
fn matches_raw_export_from_the_same_udro_layout() {
	const UDRO_IMAGE: &str = "wrapped.dmg";
	const EXPORTED: &str = "exported.raw";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	fs::write(root.join(MANIFEST_NAME), SETTINGS).unwrap();
	cli(root, &["build", MANIFEST_NAME]);
	cli(
		root,
		&[
			"build",
			MANIFEST_NAME,
			"--set",
			"image.format=udro",
			"--set",
			"image.output=wrapped.dmg",
		],
	);
	cli(root, &["unwrap-raw", UDRO_IMAGE, "--output", EXPORTED]);
	let raw = fs::read(root.join(RAW_IMAGE)).unwrap();
	assert_eq!(raw, fs::read(root.join(EXPORTED)).unwrap());
	let inspect = Command::new(BINARY)
		.current_dir(root)
		.args(["inspect", RAW_IMAGE])
		.output()
		.unwrap();
	assert!(!inspect.status.success());
	assert!(String::from_utf8_lossy(&inspect.stderr).contains("missing koly trailer"));
	#[cfg(target_os = "macos")]
	{
		const APPLE_IMAGE: &str = "apple-udrw.dmg";
		let output = Command::new("/usr/bin/hdiutil")
			.current_dir(root)
			.args([
				"convert",
				UDRO_IMAGE,
				"-format",
				"UDRW",
				"-o",
				APPLE_IMAGE,
				"-quiet",
			])
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		assert_eq!(raw, fs::read(root.join(APPLE_IMAGE)).unwrap());
	}
}
