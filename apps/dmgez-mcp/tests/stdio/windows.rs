use std::{fs, os::windows::fs::MetadataExt as _, path::Path, process::Command};

use serde_json::json;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use super::{Client, VALID_PROJECT};

const EXACT_FILE: &str = "Example.toml";
const CASE_ALIAS: &str = "example.toml";
const STREAM_NAME: &str = "Example.toml:alternate.toml";
const JUNCTION_ENTRY: &str = "linked.toml";
const ROOT_JUNCTION: &str = "project-alias";
const PROJECT_DIR: &str = "projects";
const OUTSIDE_DIR: &str = "outside";
const INSIDE_JUNCTION: &str = "projects\\linked.toml";

#[test]
fn admits_only_exact_ntfs_names_and_keeps_reparse_points_outside_the_root() {
	let temp = tempfile::tempdir().unwrap();
	let physical = temp.path().canonicalize().unwrap();
	let root = physical.join(PROJECT_DIR);
	fs::create_dir(&root).unwrap();
	let outside = physical.join(OUTSIDE_DIR);
	fs::create_dir(&outside).unwrap();
	fs::write(outside.join(EXACT_FILE), VALID_PROJECT).unwrap();
	fs::write(root.join(EXACT_FILE), VALID_PROJECT).unwrap();
	fs::write(root.join(STREAM_NAME), VALID_PROJECT).unwrap();
	junction(temp.path(), ROOT_JUNCTION, OUTSIDE_DIR);
	junction(temp.path(), INSIDE_JUNCTION, OUTSIDE_DIR);

	let mut client = Client::start(&temp.path().join(PROJECT_DIR));
	assert_eq!(client.validate(EXACT_FILE)["status"], "valid");
	assert_eq!(
		client.validate(CASE_ALIAS)["issue"]["code"],
		"project_unavailable"
	);
	assert_eq!(
		client.validate(STREAM_NAME)["issue"]["code"],
		"invalid_file_name"
	);
	assert_eq!(
		client.validate(JUNCTION_ENTRY)["issue"]["code"],
		"project_unavailable"
	);
	assert_eq!(client.list(json!({}))["file_names"], json!([EXACT_FILE]));
	client.finish();
	refuses_root(&physical.join(ROOT_JUNCTION));
}

#[test]
fn rejects_unc_and_device_roots_at_startup() {
	for root in [
		r"\\example.invalid\projects",
		r"\\.\C:\projects",
		r"C:projects",
	] {
		refuses_root(Path::new(root));
	}
}

fn junction(directory: &Path, name: &str, target: &str) {
	let junction = Command::new("cmd.exe")
		.args(["/d", "/c", "mklink", "/J", name, target])
		.current_dir(directory)
		.output()
		.unwrap();
	assert!(
		junction.status.success(),
		"could not create NTFS fixture junction: {}",
		String::from_utf8_lossy(&junction.stderr)
	);
	assert!(
		fs::symlink_metadata(directory.join(name))
			.unwrap()
			.file_attributes()
			& FILE_ATTRIBUTE_REPARSE_POINT
			!= 0,
		"junction fixture must be a reparse point"
	);
}

fn refuses_root(root: &Path) {
	let startup = Command::new(env!("CARGO_BIN_EXE_dmgez-mcp"))
		.arg("--project-root")
		.arg(root)
		.output()
		.unwrap();
	assert!(!startup.status.success());
	assert!(startup.stdout.is_empty());
}
