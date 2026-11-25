#![cfg(any(
	not(feature = "hfsplus-extract"),
	target_os = "linux",
	target_os = "macos"
))]

use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
#[path = "extract_cli/native.rs"]
mod native;

#[cfg(all(
	feature = "hfsplus-extract",
	feature = "project-build",
	any(target_os = "linux", target_os = "macos"),
))]
#[path = "extract_cli/project.rs"]
mod project;

#[cfg(all(feature = "hfsplus-extract", target_os = "macos"))]
#[path = "extract_cli/quarantine.rs"]
mod quarantine;

fn extract(image: &Path, source: &str, destination: &Path, args: &[&str]) -> Output {
	Command::new(BINARY)
		.arg("extract")
		.arg(image)
		.arg(source)
		.arg("--output")
		.arg(destination)
		.args(args)
		.output()
		.unwrap()
}

#[cfg(not(feature = "hfsplus-extract"))]
#[test]
fn reports_a_disabled_extractor_before_opening_an_image_or_destination() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("absent.dmg");
	let destination = directory.path().join("untouched");
	let output = extract(
		&absent,
		"/payload",
		&destination,
		&["--fork", "data", "--dry-run"],
	);
	assert!(!output.status.success());
	assert!(output.stdout.is_empty());
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains("hfsplus-extract"), "{diagnostic}");
	assert!(!destination.exists());
	assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

	let capabilities = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(capabilities.status.success());
	assert!(
		String::from_utf8_lossy(&capabilities.stdout)
			.lines()
			.any(|line| line == "hfsplus-extract = not_compiled"),
		"{capabilities:?}",
	);
}

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
fn success(output: Output) -> Vec<u8> {
	assert!(
		output.status.success(),
		"stdout: {}\nstderr: {}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr),
	);
	output.stdout
}

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
fn failure(output: Output) -> String {
	assert!(!output.status.success(), "{output:?}");
	let diagnostic = String::from_utf8(output.stderr).unwrap();
	assert!(!diagnostic.is_empty());
	diagnostic
}

#[cfg(all(
	feature = "hfsplus-extract",
	any(target_os = "linux", target_os = "macos")
))]
fn report(bytes: &[u8], status: &str, source: &str, destination: &Path) -> serde_json::Value {
	let report: serde_json::Value = serde_json::from_slice(bytes).unwrap();
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["operation"], "extract");
	assert_eq!(report["status"], status);
	assert_eq!(report["source_path"], source);
	assert_eq!(report["destination"], destination.to_str().unwrap());
	report
}
