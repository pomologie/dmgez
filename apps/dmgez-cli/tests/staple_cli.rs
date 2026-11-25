use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const EXIT_ERROR: i32 = 2;
const IMAGE: &str = "Signed.dmg";
const RECORD: &str = "notarization.json";
const STAPLED: &str = "Stapled.dmg";

fn staple() -> Command {
	let mut command = Command::new(BINARY);
	command.arg("staple");
	command
}

fn assert_error(output: &Output, expected: &str) {
	assert_eq!(output.status.code(), Some(EXIT_ERROR), "{output:?}");
	assert!(output.stdout.is_empty(), "{output:?}");
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

#[test]
fn documents_stapling_arguments() {
	let output = staple().arg("--help").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let help = String::from_utf8_lossy(&output.stdout);
	for element in ["<IMAGE>", "--record <PATH>", "--output <PATH>", "--json"] {
		assert!(help.contains(element), "{help}");
	}
}

#[test]
fn requires_a_record_and_output_path() {
	for (args, expected) in [
		([IMAGE, "--output", STAPLED], "--record <PATH>"),
		([IMAGE, "--record", RECORD], "--output <PATH>"),
	] {
		let output = staple().args(args).output().unwrap();
		assert_error(&output, expected);
	}
}

#[cfg(not(all(feature = "notarization-native", target_os = "macos")))]
#[test]
fn rejects_unavailable_stapling_before_accessing_paths() {
	let directory = tempfile::tempdir().unwrap();
	let diagnostic = if cfg!(feature = "notarization-native") {
		"native stapling is unavailable on this platform"
	} else {
		"feature not compiled: notarization-native"
	};
	let output = staple()
		.current_dir(directory.path())
		.args([IMAGE, "--record", RECORD, "--output", STAPLED, "--json"])
		.output()
		.unwrap();
	assert_error(&output, diagnostic);
	assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
#[test]
fn rejects_missing_and_malformed_records_before_publishing_an_image() {
	use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

	const RECORD_MODE: u32 = 0o600;
	const MALFORMED_RECORD: &[u8] = b"invalid JSON";

	let directory = tempfile::tempdir().unwrap();
	let image_path = directory.path().join(IMAGE);
	let image = b"synthetic input bytes";
	std::fs::write(&image_path, image).unwrap();

	let run = || {
		staple()
			.current_dir(directory.path())
			.args([IMAGE, "--record", RECORD, "--output", STAPLED, "--json"])
			.output()
			.unwrap()
	};
	assert_error(&run(), "notarization record");

	let record_path = directory.path().join(RECORD);
	OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(RECORD_MODE)
		.open(&record_path)
		.unwrap()
		.write_all(MALFORMED_RECORD)
		.unwrap();
	assert_error(&run(), "notarization record");

	assert_eq!(std::fs::read(image_path).unwrap(), image);
	assert_eq!(std::fs::read(record_path).unwrap(), MALFORMED_RECORD);
	assert_eq!(directory.path().read_dir().unwrap().count(), 2);
}
