use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

use super::{BINARY, MANIFEST_NAME};

const OUTPUT_NAME: &str = "Build report.dmg";
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Build report"
output = "Build report.dmg"
format = "udro"
size = "1MiB"
[[contents]]
type = "directory"
destination = "/Data"
[signing]
enabled = true
mode = "ad-hoc"
dmg = true
identifier = "org.pomologie.fixture"
"#;

#[test]
fn reports_dry_run_and_publication_as_separate_versioned_json_modes() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	fs::write(root.join(MANIFEST_NAME), SETTINGS).unwrap();
	let path = root.join(OUTPUT_NAME);
	let build = |flags: &[&str]| {
		Command::new(BINARY)
			.current_dir(&root)
			.args(["build", MANIFEST_NAME, "--json"])
			.args(flags)
			.output()
			.unwrap()
	};

	let plan = build(&["--dry-run"]);
	assert!(
		plan.status.success(),
		"{}",
		String::from_utf8_lossy(&plan.stderr)
	);
	let plan: Value = serde_json::from_slice(&plan.stdout).unwrap();
	assert_eq!(plan["schema_version"], 1);
	assert_eq!(plan["operation"], "build");
	assert_eq!(plan["mode"], "dry_run");
	assert_eq!(Path::new(plan["output"].as_str().unwrap()), path.as_path());
	assert_eq!(plan["format"], "udro");
	assert_eq!(plan["partition"], "none");
	assert_eq!(
		plan["publication"],
		json!({"kind": "path", "overwrite": false})
	);
	assert_eq!(plan["logical_bytes"], "1048576");
	assert!(plan["physical_bytes"].is_null());
	assert_eq!(plan["source_payload_bytes"], "0");
	assert_eq!(plan["release"], "configured_stages_deferred");
	assert!(!path.exists());

	let publication = build(&[]);
	assert!(
		publication.status.success(),
		"{}",
		String::from_utf8_lossy(&publication.stderr),
	);
	assert!(
		String::from_utf8_lossy(&publication.stderr)
			.contains("Configured release stages were deferred")
	);
	let publication: Value = serde_json::from_slice(&publication.stdout).unwrap();
	for field in [
		"schema_version",
		"operation",
		"format",
		"output",
		"logical_bytes",
		"release",
		"metadata_losses",
	] {
		assert_eq!(plan[field], publication[field], "field {field}");
	}
	assert_eq!(publication["mode"], "published");
	assert_eq!(publication["verified"], false);
	assert_eq!(
		publication["physical_bytes"],
		fs::metadata(&path).unwrap().len().to_string()
	);

	let refused = build(&[]);
	assert!(!refused.status.success());
	assert!(refused.stdout.is_empty());
}
