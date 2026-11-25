use std::process::{Command, Output};

const CLI: &str = env!("CARGO_BIN_EXE_dmgez");
const MISSING_IMAGE: &str = "absent-fixture.dmg";

fn require_failure(output: Output, diagnostic: &str) {
	assert!(!output.status.success(), "{output:?}");
	assert!(
		String::from_utf8_lossy(&output.stderr).contains(diagnostic),
		"{output:?}"
	);
}

#[test]
fn rejects_explicit_unavailable_driver_backends_before_opening_the_image() {
	for backend in ["fuse", "winfsp"] {
		let output = Command::new(CLI)
			.args(["mount", MISSING_IMAGE, "--backend", backend, "--detach"])
			.output()
			.unwrap();
		require_failure(
			output,
			&format!("read-only mount backend unavailable: {backend}"),
		);
	}
}

#[test]
fn recognizes_a_session_command_and_rejects_unrelated_attach_arguments() {
	let output = Command::new(CLI)
		.args(["mount", "--detach", "list"])
		.output()
		.unwrap();
	require_failure(output, "mount session commands cannot take");
}

#[cfg(not(all(feature = "mount-system", target_os = "macos")))]
#[test]
fn system_mount_and_unmount_are_gated_before_accessing_paths() {
	let expected = if cfg!(target_os = "macos") {
		"feature not compiled: mount-system"
	} else {
		"read-only mount backend unavailable: system"
	};
	for arguments in [
		&["mount", MISSING_IMAGE, "--detach"][..],
		&["mount", "list", "--json"][..],
		&["unmount", "../outside"][..],
	] {
		require_failure(
			Command::new(CLI).args(arguments).output().unwrap(),
			expected,
		);
	}
}

#[cfg(all(feature = "mount-system", target_os = "macos"))]
mod native {
	use std::{fs, path::Path};

	use dmgez_mount::session::SessionStore;
	use serde_json::Value;

	use super::*;

	const SESSION_ROOT: &str = "dmgez-mount-sessions-v1";

	fn mount_command(root: &Path, arguments: &[&str]) -> Output {
		Command::new(CLI)
			.args(arguments)
			.env("TMPDIR", root)
			.output()
			.unwrap()
	}

	fn json(output: Output) -> Value {
		assert!(output.status.success(), "{output:?}");
		serde_json::from_slice(&output.stdout).unwrap()
	}

	#[test]
	fn reports_starting_records_without_adopting_or_detaching_a_device() {
		let temporary = tempfile::tempdir().unwrap();
		let root = temporary.path().canonicalize().unwrap();
		let image = root.join("selected.dmg");
		fs::write(&image, b"synthetic record fixture").unwrap();
		let empty = json(mount_command(&root, &["mount", "list", "--json"]));
		assert_eq!(empty["sessions"], Value::Array(Vec::new()));
		let id = {
			let store = SessionStore::open(&root.join(SESSION_ROOT)).unwrap();
			store.start(&image, None).unwrap().id().to_string()
		};
		let output = json(mount_command(&root, &["mount", "status", &id, "--json"]));
		assert_eq!(output["operation"], "status");
		assert_eq!(output["sessions"].as_array().unwrap().len(), 1);
		assert!(output.get("observed").is_none());
		require_failure(
			mount_command(&root, &["unmount", &id]),
			"no confirmed attachment",
		);
	}
}
