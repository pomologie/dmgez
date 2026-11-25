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

#[cfg(all(feature = "diff", feature = "apfs-read"))]
#[path = "semantic_diff_cli/apfs.rs"]
mod apfs;
#[cfg(all(feature = "diff", feature = "apfs-read"))]
#[path = "support/native_apfs.rs"]
mod fixture;
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
fn byte_diff_rejects_the_attribute_option_before_opening_inputs() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("unused.dmg");
	for mode in ["logical", "physical"] {
		let output = Command::new(BINARY)
			.arg("diff")
			.arg(&absent)
			.arg(&absent)
			.args(["--mode", mode, XATTR_OPTION, "--json"])
			.output()
			.unwrap();
		let stderr = String::from_utf8_lossy(&output.stderr);
		assert_eq!(output.status.code(), Some(ERROR), "{stderr}");
		assert!(output.stdout.is_empty(), "{output:?}");
		assert!(
			stderr.contains("--xattrs requires --mode semantic"),
			"{stderr}"
		);
	}
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
		for options in [&[][..], &[XATTR_OPTION][..]] {
			let output = semantic_diff(&left, &right, filesystem, options);
			let stderr = String::from_utf8_lossy(&output.stderr);
			assert_eq!(output.status.code(), Some(ERROR), "{stderr}");
			assert!(output.stdout.is_empty(), "{output:?}");
			assert!(stderr.contains(&expected), "{stderr}");
		}
	}
}
