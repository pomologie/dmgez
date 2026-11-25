use super::{PROFILES, check_profile};

#[test]
fn diagnoses_package_and_feature_drift_in_existing_report_order() {
	let serial = PROFILES
		.iter()
		.find(|profile| profile.name == "serial")
		.unwrap();
	let packages = r#"dmgez-runtime v0.1.0
dmgez-cli v0.1.0
dmgez-udif v0.1.0
fuser v0.1.0
dmgez-core v0.1.0
eframe v0.1.0
dmgez-core v0.1.0 (*)
"#;
	let features = r#"dmgez-udif feature "read"
dmgez-udif feature "write" (*)
dmgez-udif feature "zlib"
"#;
	let mut errors = Vec::new();
	check_profile(serial, packages, Some(features), &mut errors);
	assert_eq!(
		errors,
		[
			"serial profile: expected ['dmgez-core', 'dmgez-runtime', 'dmgez-udif', 'flate2'], found ['dmgez-core', 'dmgez-runtime', 'dmgez-udif']",
			"serial profile: unexpected activation state for read",
			"serial profile: forbidden dependencies ['eframe', 'fuser']",
		],
	);
}

#[test]
fn does_not_treat_package_paths_or_other_crates_as_udif_features() {
	let serial = PROFILES
		.iter()
		.find(|profile| profile.name == "serial")
		.unwrap();
	let packages = r#"dmgez-cli v0.1.0
dmgez-udif v0.1.0
dmgez-runtime v0.1.0
dmgez-core v0.1.0
flate2 v0.1.0
"#;
	let features = r#"dmgez-udif v0.1.0 (/checkouts/dmgez-udif feature "lzfse")
dmgez-udif feature "write"
dmgez-udif feature "zlib"
dmgez-core feature "parallel"
"#;
	let mut errors = Vec::new();
	check_profile(serial, packages, Some(features), &mut errors);
	assert!(errors.is_empty(), "{errors:#?}");
}
