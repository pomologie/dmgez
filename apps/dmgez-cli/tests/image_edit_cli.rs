use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const ORIGINAL: &str = "original.dmg";
const CHANGES: &str = "changes.toml";
const EDITED: &str = "edited.dmg";

fn edit(root: &Path, target: &str, options: &[&str]) -> Output {
	edit_from(root, CHANGES, target, options)
}

fn edit_from(root: &Path, batch: &str, target: &str, options: &[&str]) -> Output {
	Command::new(BINARY)
		.current_dir(root)
		.args([
			"edit-image",
			ORIGINAL,
			"--changes",
			batch,
			"--output",
			target,
		])
		.args(options)
		.output()
		.unwrap()
}

fn assert_failure(output: Output, message: &str) {
	assert!(!output.status.success(), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	assert!(
		String::from_utf8_lossy(&output.stderr).contains(message),
		"{output:?}"
	);
}

#[cfg(not(feature = "image-edit"))]
#[test]
fn existing_image_edit_requires_its_composed_feature_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	assert_failure(edit(directory.path(), EDITED, &[]), "image-edit");
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(feature = "image-edit")]
#[path = "image_edit_cli/rebuild.rs"]
mod rebuild;

#[cfg(all(feature = "image-edit", feature = "finder-write"))]
#[path = "image_edit_cli/finder.rs"]
mod finder;

#[cfg(feature = "image-edit")]
#[path = "image_edit_cli/host_copy.rs"]
mod host_copy;
