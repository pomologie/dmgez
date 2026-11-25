use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const BUNDLE: &str = "Witness.app";
const APP: &str = "/Witness.app";

fn install(image: &Path, directory: &Path, arguments: &[&str]) -> Output {
	Command::new(BINARY)
		.arg("install")
		.arg(image)
		.args(["--app", APP, "--destination"])
		.arg(directory)
		.args(arguments)
		.output()
		.unwrap()
}

#[cfg(not(all(
	feature = "hfsplus-install",
	feature = "apfs-install",
	any(target_os = "linux", target_os = "macos")
)))]
#[test]
fn refuses_unavailable_installation_before_opening_host_paths() {
	let temporary = tempfile::tempdir().unwrap();
	let image = temporary.path().join("absent.dmg");
	let destination = temporary.path().join(BUNDLE);
	let capabilities = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(capabilities.status.success(), "{capabilities:?}");
	let capabilities = String::from_utf8(capabilities.stdout).unwrap();
	for (filesystem, feature, compiled) in [
		(
			"hfsplus",
			"hfsplus-install",
			cfg!(feature = "hfsplus-install"),
		),
		("apfs", "apfs-install", cfg!(feature = "apfs-install")),
	] {
		if compiled && cfg!(any(target_os = "linux", target_os = "macos")) {
			continue;
		}
		let result = install(
			&image,
			&destination,
			&["--filesystem", filesystem, "--json"],
		);
		assert!(!result.status.success(), "{result:?}");
		assert!(result.stdout.is_empty(), "{result:?}");
		let text = String::from_utf8(result.stderr).unwrap();
		let expected = if compiled {
			"requires a macOS or Linux host"
		} else {
			feature
		};
		assert!(text.contains(expected), "{text}");
		assert!(
			capabilities
				.lines()
				.any(|line| line == format!("{feature} = not_compiled"))
		);
	}
	assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[cfg(all(
	feature = "apfs-install",
	any(target_os = "linux", target_os = "macos")
))]
#[test]
fn compiled_apfs_installation_reports_an_unopenable_source_without_staging() {
	let temporary = tempfile::tempdir().unwrap();
	let image = temporary.path().join("absent.raw");
	let output = install(
		&image,
		temporary.path(),
		&["--filesystem", "apfs", "--raw", "--json"],
	);
	assert!(!output.status.success(), "{output:?}");
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["status"], "failed");
	assert_eq!(report["published"], false);
	assert!(report["residual_stage_name"].is_null());
	assert!(!temporary.path().join(BUNDLE).exists());
	assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[cfg(all(
	feature = "hfsplus-install",
	feature = "project-build",
	any(target_os = "linux", target_os = "macos")
))]
#[path = "install_cli/project.rs"]
mod project;
