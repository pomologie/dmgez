use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const ERROR: i32 = 2;
#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
const EQUAL: i32 = 0;
#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
const DIFFERENT: i32 = 1;
const XATTR_OPTION: &str = "--xattrs";
const FINDER_INFO_OPTION: &str = "--finder-info";
const CONTENTS_OPTION: &str = "--contents";
const EXACT_CONTENTS: &str = "exact";
#[cfg(feature = "diff")]
const BLAKE3_CONTENTS: &str = "blake3";
#[cfg(feature = "diff")]
const BLAKE3_EXACT_CONTENTS: &str = "blake3-exact";
#[cfg(feature = "diff")]
const INFER_RENAMES_OPTION: &str = "--infer-renames";
#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
const FINDER_INFO_FIELD: &str = "finder_info";
#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
const FINDER_INFO_BYTES: usize = 32;
#[cfg(feature = "diff")]
const FINDER_LAYOUT_OPTION: &str = "--finder-layout";

#[cfg(all(feature = "diff", feature = "apfs-read"))]
#[path = "semantic_diff_cli/apfs.rs"]
mod apfs;
#[cfg(all(
	feature = "finder-diff",
	feature = "finder-write",
	feature = "hfsplus-read",
	feature = "project-build"
))]
#[path = "semantic_diff_cli/finder_layout.rs"]
mod finder_layout;
#[cfg(all(feature = "diff", feature = "apfs-read"))]
#[path = "support/native_apfs.rs"]
mod fixture;
#[cfg(all(
	feature = "diff-hash",
	feature = "hfsplus-read",
	feature = "project-build"
))]
#[path = "semantic_diff_cli/hash_contents.rs"]
mod hash_contents;
#[cfg(all(feature = "diff", feature = "hfsplus-read"))]
#[path = "semantic_diff_cli/hfsplus.rs"]
mod hfsplus;
#[cfg(all(feature = "diff", feature = "apfs-read"))]
#[path = "support/native_apfs_mutations.rs"]
mod native_apfs_mutations;
#[cfg(all(feature = "diff", feature = "hfsplus-read"))]
#[path = "support/native_hfsplus.rs"]
mod native_hfsplus;

fn semantic_diff(left: &Path, right: &Path, filesystem: &str, options: &[&str]) -> Output {
	semantic_command(left, right, filesystem)
		.arg("--json")
		.args(options)
		.output()
		.unwrap()
}

fn semantic_command(left: &Path, right: &Path, filesystem: &str) -> Command {
	let mut command = Command::new(BINARY);
	command.arg("diff").arg(left).arg(right).args([
		"--mode",
		"semantic",
		"--filesystem",
		filesystem,
	]);
	command
}

#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
fn report(output: &Output, exit: i32) -> serde_json::Value {
	assert_eq!(
		output.status.code(),
		Some(exit),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(parsed["schema_version"], 1, "{parsed}");
	assert_eq!(parsed["address_space"], "filesystem", "{parsed}");
	parsed
}

#[cfg(all(feature = "diff", any(feature = "hfsplus-read", feature = "apfs-read")))]
#[test]
fn semantic_diff_refuses_a_nonregular_input_before_opening_it() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("unused.dmg");
	for (filesystem, available) in [
		("hfsplus", cfg!(feature = "hfsplus-read")),
		("apfs", cfg!(feature = "apfs-read")),
	] {
		if available {
			let output = semantic_diff(directory.path(), &absent, filesystem, &[]);
			let stderr = String::from_utf8_lossy(&output.stderr);
			assert_eq!(output.status.code(), Some(ERROR), "{stderr}");
			assert!(output.stdout.is_empty());
			assert!(
				stderr.contains("comparison input is not a regular file"),
				"{stderr}"
			);
		}
	}
}

#[cfg(feature = "diff")]
#[test]
fn byte_diff_rejects_semantic_options_before_opening_inputs() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("unused.dmg");
	for mode in ["logical", "physical"] {
		for options in [
			&[XATTR_OPTION][..],
			&[FINDER_LAYOUT_OPTION][..],
			&[FINDER_INFO_OPTION][..],
			&[CONTENTS_OPTION, EXACT_CONTENTS][..],
			&[CONTENTS_OPTION, BLAKE3_CONTENTS][..],
			&[CONTENTS_OPTION, BLAKE3_EXACT_CONTENTS][..],
			&[INFER_RENAMES_OPTION][..],
		] {
			let output = Command::new(BINARY)
				.arg("diff")
				.arg(&absent)
				.arg(&absent)
				.args(["--mode", mode, "--json"])
				.args(options)
				.output()
				.unwrap();
			assert_error(&output, &format!("{} requires --mode semantic", options[0]));
		}
	}
}

#[cfg(all(feature = "diff", not(feature = "diff-hash")))]
#[test]
fn unavailable_hash_and_rename_scopes_refuse_before_opening_inputs() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("unused.dmg");
	for options in [
		&[CONTENTS_OPTION, BLAKE3_CONTENTS][..],
		&[CONTENTS_OPTION, BLAKE3_EXACT_CONTENTS][..],
		&[INFER_RENAMES_OPTION][..],
	] {
		let output = semantic_diff(&absent, &absent, "hfsplus", options);
		assert_error(&output, "feature not compiled: diff-hash");
	}
}

#[cfg(all(feature = "diff", not(feature = "finder-diff")))]
#[test]
fn unavailable_finder_comparison_refuses_before_opening_inputs() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("unused.dmg");
	let output = semantic_diff(&absent, &absent, "hfsplus", &[FINDER_LAYOUT_OPTION]);
	assert_error(&output, "feature not compiled: finder-diff");
}

fn assert_error(output: &Output, expected: &str) {
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert_eq!(output.status.code(), Some(ERROR), "{stderr}");
	assert!(output.stdout.is_empty(), "{output:?}");
	assert!(stderr.contains(expected), "{stderr}");
}

#[cfg(not(all(feature = "diff", feature = "hfsplus-read", feature = "apfs-read")))]
#[test]
fn unavailable_semantic_backend_refuses_before_opening_inputs() {
	let directory = tempfile::tempdir().unwrap();
	let left = directory.path().join("unused-left.dmg");
	let right = directory.path().join("unused-right.dmg");
	let unavailable = [
		(
			"hfsplus",
			cfg!(all(feature = "diff", feature = "hfsplus-read")),
		),
		("apfs", cfg!(all(feature = "diff", feature = "apfs-read"))),
	];
	for (filesystem, compiled) in unavailable {
		if compiled {
			continue;
		}
		let expected = if cfg!(feature = "diff") {
			format!("requires the composed {filesystem}-read profile")
		} else {
			String::from("feature not compiled: diff")
		};
		for options in [
			&[][..],
			&[XATTR_OPTION][..],
			&[FINDER_INFO_OPTION][..],
			&[CONTENTS_OPTION, EXACT_CONTENTS][..],
		] {
			let output = semantic_diff(&left, &right, filesystem, options);
			assert_error(&output, &expected);
		}
	}
}
