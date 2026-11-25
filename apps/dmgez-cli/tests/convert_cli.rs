use std::{fs, process::Command};

const NATIVE_UDZO: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO.dmg");
const NATIVE_UDRO: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDRO.dmg");
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

#[cfg(feature = "convert")]
#[test]
fn signed_conversion_requires_permission_and_publishes_exact_unsigned_bytes() {
	use dmgez_udif::reader::{Image, Limits};

	const SIGNED: &[u8] =
		include_bytes!("../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
	const RAW: &[u8] = include_bytes!("../../../tests/fixtures/expected/tiny.raw");
	const PERMISSION: &str = "--allow-signature-loss";
	let work = tempfile::tempdir().unwrap();
	let source = work.path().join("signed.dmg");
	let destination = work.path().join("converted.dmg");
	fs::write(&source, SIGNED).unwrap();
	let invoke = |options: &[&str]| {
		Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.arg("convert")
			.arg(&source)
			.args(["--format", CONVERSION_FORMAT, "--serial", "-o"])
			.arg(&destination)
			.args(options)
			.output()
			.unwrap()
	};
	let denied = invoke(&[]);
	assert!(!denied.status.success(), "{denied:?}");
	assert!(denied.stdout.is_empty());
	assert!(String::from_utf8_lossy(&denied.stderr).contains("requires explicit permission"));
	assert!(!destination.exists());
	let optimized = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("optimize")
		.arg(&source)
		.args(["--serial", "-o"])
		.arg(&destination)
		.output()
		.unwrap();
	assert!(!optimized.status.success(), "{optimized:?}");
	assert!(!destination.exists());
	let converted = invoke(&[PERMISSION]);
	assert!(converted.status.success(), "{converted:?}");
	let report = String::from_utf8(converted.stdout).unwrap();
	for line in [
		"trust = SignatureAndAnyTicketRemoved",
		"output_check = VerifiedAndCompared",
	] {
		assert!(report.lines().any(|value| value == line), "{report}");
	}
	let stored = fs::read(&destination).unwrap();
	let mut image = Image::open(stored.as_slice(), Limits::default()).unwrap();
	assert!(image.embedded_signature_range().is_none());
	image.verify().unwrap();
	let mut logical = Vec::new();
	image.write_raw_to(&mut logical).unwrap();
	assert_eq!(logical, RAW);
	assert!(!invoke(&[PERMISSION]).status.success());
	assert_eq!(fs::read(destination).unwrap(), stored);
	assert_eq!(fs::read(source).unwrap(), SIGNED);
	assert_eq!(fs::read_dir(work.path()).unwrap().count(), 2);
}

#[test]
fn optimize_reports_sample_costs_and_refuses_an_exhausted_budget() {
	const FULL_SAMPLE: &str = "2048";
	const EMPTY_BUDGET: &str = "0";
	let work = tempfile::tempdir().unwrap();
	let source = work.path().join("source.dmg");
	let destination = work.path().join("optimized.dmg");
	fs::write(&source, NATIVE_UDRO).unwrap();
	let run = |budget: &str| {
		Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.arg("optimize")
			.arg(&source)
			.args([
				"--serial",
				"--max-sample-chunks",
				"1",
				"--max-sample-bytes",
				budget,
				"-o",
			])
			.arg(&destination)
			.output()
			.unwrap()
	};
	let failed = run(EMPTY_BUDGET);
	assert!(!failed.status.success());
	assert!(!destination.exists());
	let output = run(FULL_SAMPLE);
	assert_eq!(output.status.success(), cfg!(feature = "convert"));
	assert_eq!(destination.exists(), cfg!(feature = "convert"));
	if cfg!(feature = "convert") {
		assert!(String::from_utf8_lossy(&failed.stderr).contains("sample byte budget"));
		let stdout = String::from_utf8(output.stdout).unwrap();
		assert!(stdout.lines().any(|line| line == "sampled_chunks = 1"));
		assert!(
			stdout
				.lines()
				.any(|line| line == "sample = offset 0 bytes 2048")
		);
		assert!(
			stdout
				.lines()
				.any(|line| line.starts_with("candidate = Udro wrapped_bytes "))
		);
		assert!(
			stdout
				.lines()
				.any(|line| line.starts_with("candidate = Udzo wrapped_bytes "))
		);
		assert!(stdout.lines().any(|line| line == "output_profile = Udzo"));
		assert!(
			stdout
				.lines()
				.any(|line| line == "output_check = VerifiedAndCompared")
		);
	} else {
		assert!(String::from_utf8_lossy(&output.stderr).contains("feature not compiled: convert"));
	}
	assert_eq!(fs::read(source).unwrap(), NATIVE_UDRO);
}
