use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const FEATURE: &str = "installer-test";
const PROFILE: &str = "installer";
const IMAGE_NAME: &str = "installer.dmg";

fn installer() -> Command {
	let mut command = Command::new(BINARY);
	command.args(["test", IMAGE_NAME, "--profile", PROFILE, "--json"]);
	command
}

#[test]
fn reports_the_selected_installer_test_capability() {
	let output = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let report = String::from_utf8(output.stdout).unwrap();
	let compiled = cfg!(all(
		feature = "installer-test",
		any(feature = "hfsplus-read", feature = "apfs-read"),
		any(target_os = "linux", target_os = "macos")
	));
	let expected = if compiled { "compiled" } else { "not_compiled" };
	assert!(
		report
			.lines()
			.any(|line| line == format!("{FEATURE} = {expected}")),
		"{report}"
	);
}

#[cfg(not(all(
	feature = "installer-test",
	any(feature = "hfsplus-read", feature = "apfs-read"),
	any(target_os = "linux", target_os = "macos")
)))]
#[test]
fn rejects_the_missing_profile_before_opening_a_path() {
	let directory = tempfile::tempdir().unwrap();
	let output = installer().current_dir(directory.path()).output().unwrap();
	assert_eq!(output.status.code(), Some(2), "{output:?}");
	assert!(output.stdout.is_empty());
	assert!(String::from_utf8_lossy(&output.stderr).contains(FEATURE));
}

#[cfg(all(
	feature = "installer-test",
	any(feature = "hfsplus-read", feature = "apfs-read"),
	any(target_os = "linux", target_os = "macos")
))]
fn report(output: std::process::Output, exit: i32) -> serde_json::Value {
	assert_eq!(output.status.code(), Some(exit), "{output:?}");
	let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(result["schema_version"], 1);
	assert_eq!(result["profile"], PROFILE);
	result
}

#[cfg(all(
	feature = "installer-test",
	any(feature = "hfsplus-read", feature = "apfs-read"),
	any(target_os = "linux", target_os = "macos")
))]
#[test]
fn invalid_container_blocks_the_requested_release_checks() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	std::fs::write(root.join(IMAGE_NAME), b"not a disk image").unwrap();
	let result = report(
		installer()
			.current_dir(root)
			.arg("--require-native-assessment")
			.output()
			.unwrap(),
		1,
	);
	assert_eq!(result["container"]["status"], "failed");
	assert_eq!(result["native_assessment"]["status"], "blocked");
}

#[cfg(all(
	feature = "installer-test",
	feature = "project-build",
	feature = "hfsplus-read",
	any(target_os = "linux", target_os = "macos")
))]
#[path = "installer_test_cli/hfsplus.rs"]
mod hfsplus;
