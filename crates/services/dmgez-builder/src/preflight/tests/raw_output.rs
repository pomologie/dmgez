use super::*;

#[test]
fn admits_udrw_geometry_without_a_udif_run_budget() {
	let mut project = project("[performance]\nmax_runs = 1\n");
	project.image.format = ImageFormat::Udrw;
	let raw = preflight(&project, options()).unwrap();
	assert_eq!(raw.format(), ImageFormat::Udrw);
	assert_eq!(raw.write_options().execution, Execution::Serial);
	raw.validate_logical_bytes(4 * MIB).unwrap();
	for bytes in [0, 1, raw.max_logical_bytes() + MIB] {
		assert!(matches!(
			raw.validate_logical_bytes(bytes),
			Err(PreflightError::UdrwGeometry)
		));
	}
}

#[test]
fn rejects_incompatible_udrw_options_without_opening_sources() {
	let fixture = |settings| {
		let mut project = project(settings);
		project.image.format = ImageFormat::Udrw;
		project
	};
	for settings in [
		"[signing]\nenabled = true\ndmg = true\n",
		"[notarization]\nenabled = true\n",
		"[release]\nverify = true\n",
	] {
		assert!(matches!(
			preflight(&fixture(settings), options()),
			Err(PreflightError::UdrwRelease)
		));
	}
	let mut release = options();
	release.release = ReleaseIntent::Release;
	assert!(matches!(
		preflight(&fixture(""), release),
		Err(PreflightError::UdrwRelease)
	));
	assert!(matches!(
		preflight(&fixture(CACHE), options()),
		Err(PreflightError::CacheFormat)
	));
	assert!(matches!(
		preflight(&fixture("[build]\nverify_output = true\n"), options()),
		Err(PreflightError::UdrwVerification)
	));
	assert!(matches!(
		preflight(&fixture("[performance]\nparallel = true\n"), options()),
		Err(PreflightError::UdrwParallel)
	));
}
