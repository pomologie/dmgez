use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const EXIT_ERROR: i32 = 2;
const IMAGE: &str = "Signed.dmg";
const RECORD: &str = "notarization.json";
const PROFILE: &str = "mock-credential-profile";
const STATUS: &str = "status";
const UUID: &str = "5ed04b9e-618b-419e-bdbe-32b2874f80ad";

fn notarize() -> Command {
	let mut command = Command::new(BINARY);
	command.arg("notarize");
	command
}

fn assert_error(output: &Output, expected: &str) {
	assert_eq!(output.status.code(), Some(EXIT_ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

#[test]
fn reports_notarization_capability() {
	let output = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let expected = match (
		cfg!(feature = "notarization-native"),
		cfg!(target_os = "macos"),
	) {
		(false, _) => "notarization-native = not_compiled",
		(true, false) => "notarization-native = unavailable",
		(true, true) => "notarization-native = compiled",
	};
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(stdout.lines().any(|line| line == expected), "{stdout}");
}

#[test]
fn documents_submission_and_status_arguments() {
	for (command, operand) in [(&[][..], "<IMAGE>"), (&[STATUS][..], "<ID>")] {
		let output = notarize().args(command).arg("--help").output().unwrap();
		assert!(output.status.success(), "{output:?}");
		let help = String::from_utf8_lossy(&output.stdout);
		for element in [
			operand,
			"--record <PATH>",
			"--keychain-profile <NAME>",
			"--wait",
			"--json",
		] {
			assert!(help.contains(element), "{help}");
		}
		assert!(
			help.contains("Apple continues processing after the local wait ends."),
			"{help}"
		);
	}
}

#[test]
fn requires_a_record_for_submission() {
	let output = notarize()
		.args([IMAGE, "--keychain-profile", PROFILE])
		.output()
		.unwrap();
	assert_error(&output, "--record <PATH>");
}

#[cfg(not(all(feature = "notarization-native", target_os = "macos")))]
#[test]
fn refuses_unavailable_notarization_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let expected = if cfg!(feature = "notarization-native") {
		"unavailable on this platform"
	} else {
		"feature not compiled: notarization-native"
	};
	for args in [&[IMAGE][..], &[STATUS, UUID][..]] {
		let output = notarize()
			.current_dir(directory.path())
			.args(args)
			.args([
				"--record",
				RECORD,
				"--keychain-profile",
				PROFILE,
				"--wait",
				"--json",
			])
			.output()
			.unwrap();
		assert_error(&output, expected);
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
#[test]
fn rejects_malformed_submission_id_before_accessing_record() {
	let directory = tempfile::tempdir().unwrap();
	for record in [&[][..], &["--record", RECORD][..]] {
		let output = notarize()
			.current_dir(directory.path())
			.args([
				STATUS,
				"not-a-submission-id",
				"--keychain-profile",
				PROFILE,
				"--json",
			])
			.args(record)
			.output()
			.unwrap();
		assert_error(&output, "UUID");
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
#[test]
fn rejects_invalid_profile_before_accessing_paths_and_keeps_it_out_of_diagnostics() {
	const INVALID_PROFILE_ARG: &str = "--keychain-profile=invalid-synthetic-profile\n";
	const INVALID_PROFILE_VALUE: &str = "invalid-synthetic-profile";

	let directory = tempfile::tempdir().unwrap();
	for args in [&[IMAGE][..], &[STATUS, UUID][..]] {
		let output = notarize()
			.current_dir(directory.path())
			.args(args)
			.args(["--record", RECORD, INVALID_PROFILE_ARG, "--json"])
			.output()
			.unwrap();
		assert_error(&output, "profile");
		assert!(!String::from_utf8_lossy(&output.stderr).contains(INVALID_PROFILE_VALUE));
	}
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}
