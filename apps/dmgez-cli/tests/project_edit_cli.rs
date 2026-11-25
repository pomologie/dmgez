use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const PROJECT: &str = "project/dmg.toml";
const BATCH: &str = "changes.toml";
const EDITED_IMAGE: &str = "edited.dmg";

fn edit(root: &Path, output: &str, options: &[&str]) -> Output {
	Command::new(BINARY)
		.current_dir(root)
		.args(["edit", PROJECT, "--changes", BATCH, "--output", output])
		.args(options)
		.output()
		.unwrap()
}

#[cfg(not(feature = "project-edit"))]
#[test]
fn edit_requires_its_composed_profile_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let output = edit(directory.path(), EDITED_IMAGE, &[]);
	assert!(!output.status.success(), "{output:?}");
	assert!(String::from_utf8_lossy(&output.stderr).contains("project-edit"));
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(feature = "project-edit")]
#[path = "project_edit_cli/rebuild.rs"]
mod rebuild;
