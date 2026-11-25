use std::{fs, process::Command};

const NATIVE_UDZO: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO.dmg");
const CONVERSION_FORMAT: &str = "udro";
const EXISTING: &[u8] = b"keep this entry";

#[test]
fn creates_a_verified_conversion_only_when_the_feature_is_compiled() {
	let work = tempfile::tempdir().unwrap();
	let source = work.path().join("source.dmg");
	let destination = work.path().join("converted.dmg");
	fs::write(&source, NATIVE_UDZO).unwrap();
	let convert = || {
		Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.arg("convert")
			.arg(&source)
			.args(["--format", CONVERSION_FORMAT, "--serial", "-o"])
			.arg(&destination)
			.output()
			.unwrap()
	};
	let output = convert();
	assert_eq!(output.status.success(), cfg!(feature = "convert"));
	assert_eq!(destination.exists(), cfg!(feature = "convert"));
	if cfg!(feature = "convert") {
		let stdout = String::from_utf8(output.stdout).unwrap();
		assert!(stdout.lines().any(|line| line == "output_profile = Udro"));
		assert!(
			stdout
				.lines()
				.any(|line| line == "output_check = VerifiedAndCompared")
		);
		assert!(
			stdout
				.lines()
				.any(|line| line == "trust = UnsignedPreserved")
		);
	} else {
		assert!(String::from_utf8_lossy(&output.stderr).contains("feature not compiled: convert"));
	}
	#[cfg(feature = "convert")]
	{
		use dmgez_udif::reader::{Image, Limits};

		let converted = fs::read(&destination).unwrap();
		let mut converted = Image::open(converted.as_slice(), Limits::default()).unwrap();
		converted.verify().unwrap();
		let mut native = Image::open(NATIVE_UDZO, Limits::default()).unwrap();
		let mut native_bytes = Vec::new();
		let mut converted_bytes = Vec::new();
		native.write_raw_to(&mut native_bytes).unwrap();
		converted.write_raw_to(&mut converted_bytes).unwrap();
		assert_eq!(converted_bytes, native_bytes);
	}
	fs::write(&destination, EXISTING).unwrap();
	assert!(!convert().status.success());
	assert_eq!(fs::read(destination).unwrap(), EXISTING);
	assert_eq!(fs::read(source).unwrap(), NATIVE_UDZO);
	assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
}
