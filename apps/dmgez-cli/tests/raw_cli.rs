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

#[cfg(feature = "build")]
#[test]
fn raw_ulmo_wrap_requires_the_optional_codec() {
	const COMPRESSIBLE_BYTES: usize = 64 * 1024;
	let work = tempfile::tempdir().unwrap();
	let source = work.path().join("source.raw");
	let destination = work.path().join("output.dmg");
	fs::write(&source, vec![8; COMPRESSIBLE_BYTES]).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("wrap-raw")
		.arg(source)
		.args(["--format", "ulmo", "-o"])
		.arg(&destination)
		.output()
		.unwrap();
	assert_eq!(output.status.success(), cfg!(feature = "lzma"));
	assert_eq!(destination.exists(), cfg!(feature = "lzma"));
	if !cfg!(feature = "lzma") {
		assert!(String::from_utf8_lossy(&output.stderr).contains("feature not compiled: lzma"));
	}
}

#[cfg(feature = "inspect")]
#[test]
fn reports_native_gpt_byte_ranges_without_claiming_verification() {
	const NATIVE_GPT: &[u8] =
		include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg");
	let work = tempfile::tempdir().unwrap();
	let image = work.path().join("gpt.dmg");
	fs::write(&image, NATIVE_GPT).unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("inspect")
		.arg(&image)
		.args(["--partitions", "--json"])
		.output()
		.unwrap();
	assert!(output.status.success());
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["disk"]["scheme"], "gpt");
	assert_eq!(
		report["disk"]["partitions"][0]["logical"]["offset_bytes"],
		"20480"
	);
	assert_eq!(report["coverage"]["payload"], "not_checked");
	assert_eq!(report["coverage"]["filesystem"], "not_checked");
}

#[cfg(feature = "inspect")]
#[test]
fn exports_exactly_the_selected_native_partition_without_clobbering() {
	const NATIVE_APM: &[u8] =
		include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	const NATIVE_PARTITIONLESS: &[u8] =
		include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-raw.dmg");
	// hdiutil imageinfo and UDTO export, recorded in the native map fixture ledger.
	const APM_HFS_SLOT: &str = "2";
	const APM_HFS_START: usize = 32_768;
	const APM_HFS_LEN: usize = 16_744_448;
	const ABSENT_SLOT: &str = "99";
	const ZERO_SLOT: &str = "0";
	const EXISTING_BYTES: &[u8] = b"the destination already exists";
	let work = tempfile::tempdir().unwrap();
	let image = work.path().join("apm.dmg");
	let partitionless = work.path().join("partitionless.dmg");
	let whole = work.path().join("whole.raw");
	let partition = work.path().join("partition.raw");
	let rejected_output = work.path().join("rejected.raw");
	fs::write(&image, NATIVE_APM).unwrap();
	fs::write(&partitionless, NATIVE_PARTITIONLESS).unwrap();
	let export = |source: &std::path::Path, output: &std::path::Path, slot: Option<&str>| {
		let mut command = Command::new(env!("CARGO_BIN_EXE_dmgez"));
		command.arg("unwrap-raw").arg(source).arg("-o").arg(output);
		if let Some(slot) = slot {
			command.args(["--partition", slot]);
		}
		command.output().unwrap()
	};
	assert!(export(&image, &whole, None).status.success());
	assert!(
		export(&image, &partition, Some(APM_HFS_SLOT))
			.status
			.success()
	);
	let whole_bytes = fs::read(&whole).unwrap();
	assert_eq!(
		fs::read(&partition).unwrap(),
		whole_bytes[APM_HFS_START..APM_HFS_START + APM_HFS_LEN]
	);
	for (input, slot) in [
		(&image, ABSENT_SLOT),
		(&image, ZERO_SLOT),
		(&partitionless, APM_HFS_SLOT),
	] {
		assert!(!export(input, &rejected_output, Some(slot)).status.success());
		assert!(!rejected_output.exists());
	}
	fs::write(&partition, EXISTING_BYTES).unwrap();
	assert!(
		!export(&image, &partition, Some(APM_HFS_SLOT))
			.status
			.success()
	);
	assert_eq!(fs::read(&partition).unwrap(), EXISTING_BYTES);
	assert_eq!(fs::read(&image).unwrap(), NATIVE_APM);
	assert_eq!(fs::read_dir(work.path()).unwrap().count(), 4);
}

#[cfg(feature = "inspect")]
#[test]
fn analyzes_the_native_image_run_map() {
	const NATIVE_HYBRID: &str = "../../tests/fixtures/images/hybrid-UDZO.dmg";
	let image = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(NATIVE_HYBRID);
	let output = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("analyze")
		.arg(image)
		.arg("--json")
		.output()
		.unwrap();
	assert!(output.status.success());
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["operation"], "analyze");
	assert_eq!(report["container_bytes"], "6569");
	assert_eq!(report["runs"]["logical_bytes"], "921600");
	assert_eq!(report["runs"]["stored_bytes"], "918");
	assert_eq!(report["by_encoding"]["ignore"]["count"], 3);
	assert_eq!(report["coverage"]["payload"], "not_checked");
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
	let inspected = run(&[
		"inspect".as_ref(),
		image.as_os_str(),
		"--json".as_ref(),
		"--run-limit".as_ref(),
		"1".as_ref(),
	]);
	let inspected: serde_json::Value = serde_json::from_str(&inspected).unwrap();
	assert_eq!(inspected["schema_version"], 1);
	assert_eq!(inspected["container"]["logical_bytes"], "1024");
	assert_eq!(inspected["coverage"]["payload"], "not_checked");
	assert_eq!(inspected["run_page"]["runs"].as_array().unwrap().len(), 1);

	let quick = run(&[
		"verify".as_ref(),
		image.as_os_str(),
		"--quick".as_ref(),
		"--json".as_ref(),
	]);
	let quick: serde_json::Value = serde_json::from_str(&quick).unwrap();
	assert_eq!(quick["outcome"], "passed");
	assert_eq!(quick["coverage"]["structure"], "passed");
	assert_eq!(quick["coverage"]["payload"], "not_checked");

	let full = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("verify")
		.arg(&image)
		.args(["--full", "--json"])
		.output()
		.unwrap();
	assert!(!full.status.success());
	let full: serde_json::Value = serde_json::from_slice(&full.stdout).unwrap();
	assert_eq!(full["outcome"], "partial");
	assert_eq!(full["coverage"]["payload"], "passed");
	assert_eq!(full["coverage"]["filesystem"], "unavailable");
	run(&[
		"unwrap-raw".as_ref(),
		image.as_os_str(),
		"-o".as_ref(),
		export.as_os_str(),
	]);
	assert_eq!(fs::read(export).unwrap(), logical);
}
