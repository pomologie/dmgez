use std::{fs, process::Command};

#[test]
fn reports_compiled_features() {
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("capabilities")
		.output()
		.unwrap();
	assert!(output.status.success());
	let stdout = String::from_utf8(output.stdout).unwrap();
	let expected = if cfg!(feature = "build") {
		"compiled"
	} else {
		"not_compiled"
	};
	assert!(
		stdout
			.lines()
			.any(|line| line == format!("wrap-raw = {expected}"))
	);
}

#[test]
fn output_remains_unclobbered_on_conflict_or_invalid_input() {
	let work = tempfile::tempdir().unwrap();
	let raw = work.path().join("input.raw");
	let target = work.path().join("output.dmg");
	let run = |destination: &std::path::Path| {
		Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.arg("wrap-raw")
			.arg(&raw)
			.arg("--format")
			.arg("udro")
			.arg("-o")
			.arg(destination)
			.output()
			.unwrap()
	};
	fs::write(&raw, [1; 512]).unwrap();
	fs::write(&target, b"existing output").unwrap();
	let result = run(&target);
	assert!(!result.status.success());
	assert_eq!(fs::read(&target).unwrap(), b"existing output");
	if cfg!(feature = "build") {
		let result = run(&raw);
		assert!(!result.status.success());
		assert_eq!(fs::read(&raw).unwrap(), [1; 512]);
		fs::remove_file(&target).unwrap();
		fs::write(&raw, [1; 511]).unwrap();
		assert!(!run(&target).status.success());
		assert!(!target.exists());
		assert_eq!(fs::read_dir(work.path()).unwrap().count(), 1);
	} else {
		assert!(String::from_utf8_lossy(&result.stderr).contains("feature not compiled: build"));
	}
}

#[cfg(all(feature = "build", feature = "inspect"))]
#[test]
fn wraps_inspects_verifies_and_exports_through_the_cli() {
	let work = tempfile::tempdir().unwrap();
	let input = work.path().join("raw-input");
	let image = work.path().join("wrapped-without-extension");
	let export = work.path().join("export.raw");
	let logical = [11; 1024];
	fs::write(&input, logical).unwrap();
	let run = |args: &[&std::ffi::OsStr]| {
		let result = Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.args(args)
			.output()
			.unwrap();
		assert!(
			result.status.success(),
			"{}",
			String::from_utf8_lossy(&result.stderr)
		);
		String::from_utf8(result.stdout).unwrap()
	};
	run(&[
		"wrap-raw".as_ref(),
		input.as_os_str(),
		"-o".as_ref(),
		image.as_os_str(),
	]);
	let report = run(&["inspect".as_ref(), image.as_os_str()]);
	assert!(report.contains("logical_bytes = 1024"));
	assert!(report.contains("verification = not_checked"));
	let report = run(&["verify".as_ref(), image.as_os_str()]);
	assert!(report.contains("master = passed"));
	assert!(report.contains("filesystem = not_checked"));
	run(&[
		"unwrap-raw".as_ref(),
		image.as_os_str(),
		"-o".as_ref(),
		export.as_os_str(),
	]);
	assert_eq!(fs::read(export).unwrap(), logical);
}
