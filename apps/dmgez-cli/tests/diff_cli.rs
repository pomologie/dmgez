use std::{path::Path, process::Command};

const FIXTURES: &str = "../../tests/fixtures/images";
const RAW_IMAGE: &str = "tiny-UDRO.dmg";
const COMPRESSED_IMAGE: &str = "tiny-UDZO.dmg";
#[cfg(feature = "diff")]
const DIFFERENT: i32 = 1;
const ERROR: i32 = 2;

#[test]
fn reports_comparison_outcomes_and_exit_codes() {
	let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
	let left = fixtures.join(RAW_IMAGE);
	let right = fixtures.join(COMPRESSED_IMAGE);
	let run = |options: &[&str]| {
		Command::new(env!("CARGO_BIN_EXE_dmgez"))
			.arg("diff")
			.arg(&left)
			.arg(&right)
			.args(options)
			.output()
			.unwrap()
	};
	let logical = run(&["--json"]);
	#[cfg(feature = "diff")]
	{
		assert!(logical.status.success());
		let report: serde_json::Value = serde_json::from_slice(&logical.stdout).unwrap();
		assert_eq!(report["schema_version"], 1);
		assert_eq!(report["address_space"], "logical_disk");
		assert_eq!(report["outcome"], "equal");
		assert_eq!(report["common_prefix_bytes"], "2048");
		let physical = run(&["--mode", "physical", "--json"]);
		assert_eq!(physical.status.code(), Some(DIFFERENT));
		let report: serde_json::Value = serde_json::from_slice(&physical.stdout).unwrap();
		assert_eq!(report["address_space"], "physical_file");
		assert_eq!(report["outcome"], "byte_mismatch");
		let exhausted = run(&["--max-bytes", "0", "--json"]);
		assert_eq!(exhausted.status.code(), Some(ERROR));
		assert!(exhausted.stdout.is_empty());
		assert!(String::from_utf8_lossy(&exhausted.stderr).contains("without a result"));
	}
	#[cfg(not(feature = "diff"))]
	{
		assert_eq!(logical.status.code(), Some(ERROR));
		assert!(logical.stdout.is_empty());
		assert!(String::from_utf8_lossy(&logical.stderr).contains("feature not compiled: diff"));
	}
}
