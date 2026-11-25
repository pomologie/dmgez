use std::{fs, path::Path, process::Output};

use super::{BINARY, Command, MANIFEST_NAME};

const IMAGE: &str = "case-sensitive.dmg";
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Case-Sensitive Fixture"
output = "case-sensitive.dmg"
format = "udzo"
filesystem = "hfsx"
[[contents]]
source = "first.txt"
destination = "/Files/Case.txt"
[[contents]]
source = "second.txt"
destination = "/Files/case.txt"
[metadata]
preserve_timestamps = false
"#;

#[test]
fn hfsx_build_keeps_case_distinct_names_and_exact_lookup() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	write_project(root);
	let plan = success(cli(root, &["build", MANIFEST_NAME, "--dry-run", "--json"]));
	let plan: serde_json::Value = serde_json::from_slice(&plan).unwrap();
	assert_eq!(plan["filesystem"], "hfsx");
	assert!(!root.join(IMAGE).exists());
	success(cli(root, &["build", MANIFEST_NAME]));
	#[cfg(feature = "hfsplus-verify")]
	{
		let output = cli(root, &["verify", IMAGE, "--full", "--json"]);
		assert_eq!(output.status.code(), Some(1));
		let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(report["outcome"], "partial");
		assert_eq!(report["coverage"]["payload"], "passed");
		assert_eq!(
			report["filesystem_check"]["evidence"]["scope"],
			"reachable_stored_data",
		);
	}
	for (path, contents) in [
		("/Files/Case.txt", "Uppercase entry"),
		("/Files/case.txt", "Lowercase entry"),
	] {
		assert_eq!(
			success(cli(root, &["cat", IMAGE, path])),
			contents.as_bytes()
		);
	}
	assert!(
		!cli(root, &["cat", IMAGE, "/Files/CASE.txt"])
			.status
			.success()
	);
	let folded = cli(
		root,
		&[
			"build",
			MANIFEST_NAME,
			"--set",
			"image.filesystem=hfsplus",
			"--set",
			"image.output=folded.dmg",
		],
	);
	assert!(!folded.status.success(), "{folded:?}");
	assert!(!root.join("folded.dmg").exists());

	#[cfg(target_os = "macos")]
	success(
		Command::new("/usr/bin/hdiutil")
			.current_dir(root)
			.args(["verify", "-nocache", "-quiet", IMAGE])
			.output()
			.unwrap(),
	);
}

#[cfg(feature = "partition-write")]
#[test]
fn hfsx_apm_uses_the_native_partition_type() {
	const HFSX_APM_TYPE: &str = "Apple_HFSX";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	write_project(root);
	success(cli(
		root,
		&["build", MANIFEST_NAME, "--set", "image.partition=apm"],
	));
	let partitions = success(cli(root, &["inspect", IMAGE, "--partitions", "--json"]));
	let partitions: serde_json::Value = serde_json::from_slice(&partitions).unwrap();
	let volume = &partitions["disk"]["partitions"][1];
	assert_eq!(volume["details"]["type_code"], HFSX_APM_TYPE);
	assert_eq!(
		success(cli(
			root,
			&["cat", IMAGE, "/Files/case.txt", "--partition", "2"]
		)),
		b"Lowercase entry",
	);
	#[cfg(target_os = "macos")]
	{
		let map = success(
			Command::new("/usr/bin/hdiutil")
				.current_dir(root)
				.args(["pmap", IMAGE])
				.output()
				.unwrap(),
		);
		assert!(String::from_utf8(map).unwrap().contains(HFSX_APM_TYPE));
	}
}

fn write_project(root: &Path) {
	fs::write(root.join(MANIFEST_NAME), SETTINGS).unwrap();
	fs::write(root.join("first.txt"), b"Uppercase entry").unwrap();
	fs::write(root.join("second.txt"), b"Lowercase entry").unwrap();
}

fn cli(root: &Path, args: &[&str]) -> Output {
	Command::new(BINARY)
		.current_dir(root)
		.args(args)
		.output()
		.unwrap()
}

fn success(output: Output) -> Vec<u8> {
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	output.stdout
}
