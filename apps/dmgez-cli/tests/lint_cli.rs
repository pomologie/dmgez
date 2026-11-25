use std::{fs, path::Path, process::Command};

const PROJECT_FILE: &str = "project.toml";
const ERROR: i32 = 2;

fn lint(project: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_dmgez"));
	command
		.args(["lint"])
		.arg(project)
		.args(["--profile", "layout"]);
	command
}

#[cfg(not(feature = "lint"))]
#[test]
fn reports_disabled_capability_without_opening_the_project() {
	let capabilities = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("capabilities")
		.output()
		.unwrap();
	assert!(capabilities.status.success());
	assert!(String::from_utf8_lossy(&capabilities.stdout).contains("lint = not_compiled"));
	let directory = tempfile::tempdir().unwrap();
	let project = directory.path().join(PROJECT_FILE);
	let result = lint(&project).output().unwrap();
	assert_eq!(result.status.code(), Some(ERROR));
	assert!(result.stdout.is_empty());
	assert!(String::from_utf8_lossy(&result.stderr).contains("feature not compiled: lint"));
	assert!(!fs::exists(project).unwrap());
}

#[cfg(feature = "lint")]
const MANIFEST: &str = "schema_version = 1
[image]
name = 'Example'
output = 'dist/example.dmg'
[[contents]]
source = 'missing-a'
destination = '/Docs/A'
position = [90, 90]
[[contents]]
source = 'missing-b'
destination = '/Docs/B'
position = [100, 100]
[[contents]]
source = 'missing-hidden'
destination = '/Docs/Hidden'
position = [95, 100]
hidden = true
";

#[cfg(feature = "lint")]
#[test]
fn reports_and_suppresses_shared_nested_layout_findings() {
	const WARNINGS: i32 = 1;
	const REASON: &str = "intentional=overlap";
	let directory = tempfile::tempdir().unwrap();
	let project = directory.path().join(PROJECT_FILE);
	fs::write(&project, MANIFEST).unwrap();
	let run = |arguments: &[&str]| {
		lint(&project)
			.args(["--folder", "/Docs", "--json"])
			.args(arguments)
			.output()
			.unwrap()
	};
	let warnings = run(&[]);
	assert_eq!(warnings.status.code(), Some(WARNINGS));
	assert!(warnings.stderr.is_empty());
	let report: serde_json::Value = serde_json::from_slice(&warnings.stdout).unwrap();
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["profile"], "layout");
	assert_eq!(report["folder"], "/Docs");
	assert_eq!(report["diagnostics"].as_array().unwrap().len(), 2);
	assert_eq!(report["diagnostics"][0]["rule"], "hidden_positioned_icon");
	assert_eq!(report["diagnostics"][0]["severity"], "info");
	assert_eq!(report["diagnostics"][1]["rule"], "icons_overlap");
	assert_eq!(report["diagnostics"][1]["severity"], "warning");
	assert_eq!(
		report["diagnostics"][1]["evidence"]["first"]["destination"],
		"/Docs/A"
	);
	assert_eq!(
		report["diagnostics"][1]["evidence"]["second"]["destination"],
		"/Docs/B"
	);
	let assignment = format!("icons_overlap={REASON}");
	let suppressed = run(&["--suppress", &assignment]);
	assert!(suppressed.status.success());
	let report: serde_json::Value = serde_json::from_slice(&suppressed.stdout).unwrap();
	assert_eq!(report["diagnostics"][1]["suppression_reason"], REASON);
	let root = lint(&project).arg("--json").output().unwrap();
	assert_eq!(root.status.code(), Some(WARNINGS));
	let report: serde_json::Value = serde_json::from_slice(&root.stdout).unwrap();
	assert_eq!(report["folder"], "/");
	assert_eq!(report["diagnostics"].as_array().unwrap().len(), 1);
	assert_eq!(report["diagnostics"][0]["rule"], "tight_grid_spacing");
	assert_eq!(fs::read_to_string(&project).unwrap(), MANIFEST);
	assert!(!directory.path().join("dist").exists());
}

#[cfg(feature = "lint")]
#[test]
fn rejects_missing_folders_invalid_policy_and_oversized_input_without_a_report() {
	let directory = tempfile::tempdir().unwrap();
	let project = directory.path().join(PROJECT_FILE);
	fs::write(&project, MANIFEST).unwrap();
	for (arguments, message) in [
		(&["--folder", "/Docs/A"][..], "no declared project folder"),
		(
			&["--suppress", "typo=reason"][..],
			"unknown layout lint rule",
		),
		(&["--suppress", "icons_overlap= "][..], "requires a reason"),
		(
			&[
				"--suppress",
				"icons_overlap=one",
				"--suppress",
				"icons_overlap=two",
			][..],
			"multiple suppressions",
		),
	] {
		let failed = lint(&project)
			.args(arguments)
			.arg("--json")
			.output()
			.unwrap();
		assert_eq!(failed.status.code(), Some(ERROR));
		assert!(failed.stdout.is_empty());
		assert!(String::from_utf8_lossy(&failed.stderr).contains(message));
	}
	let oversized = fs::File::create(&project).unwrap();
	oversized
		.set_len(dmgez_config::MAX_MANIFEST_BYTES as u64 + 1)
		.unwrap();
	let failed = lint(&project).arg("--json").output().unwrap();
	assert_eq!(failed.status.code(), Some(ERROR));
	assert!(failed.stdout.is_empty());
	assert!(String::from_utf8_lossy(&failed.stderr).contains("input limit"));
}
