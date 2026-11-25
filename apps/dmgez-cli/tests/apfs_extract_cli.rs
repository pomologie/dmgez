//! D-READ-05 APFS extraction against independently captured native records.

#![cfg(any(
	not(feature = "apfs-extract"),
	target_os = "linux",
	target_os = "macos"
))]

use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");

#[cfg(all(
	feature = "apfs-extract",
	any(target_os = "linux", target_os = "macos")
))]
#[path = "apfs_extract_cli/native.rs"]
mod native;

fn extract(image: &Path, source: &str, destination: &Path, flags: &[&str]) -> Output {
	Command::new(BINARY)
		.arg("extract")
		.arg(image)
		.arg(source)
		.arg("--output")
		.arg(destination)
		.args(["--filesystem", "apfs"])
		.args(flags)
		.output()
		.unwrap()
}

#[cfg(not(feature = "apfs-extract"))]
#[test]
fn reports_unavailable_apfs_extraction_before_inspecting_paths() {
	let temporary = tempfile::tempdir().unwrap();
	let image = temporary.path().join("absent.raw");
	let output = temporary.path().join("untouched");
	let result = extract(&image, "/source", &output, &["--raw", "--fork", "data"]);
	assert!(!result.status.success());
	assert!(result.stdout.is_empty());
	let diagnostic = String::from_utf8_lossy(&result.stderr);
	assert!(diagnostic.contains("apfs-extract"), "{diagnostic}");
	assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);

	let capabilities = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(capabilities.status.success());
	let capabilities = String::from_utf8_lossy(&capabilities.stdout);
	assert!(
		capabilities
			.lines()
			.any(|line| line == "apfs-extract = not_compiled"),
		"{capabilities}"
	);
}
