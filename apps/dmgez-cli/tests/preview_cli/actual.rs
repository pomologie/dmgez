use std::{fs, process::Command};

use super::{
	enabled::{assert_success, decode, pixel},
	invoke_image,
};

const IMAGE: &[u8] = include_bytes!("../../../../tests/fixtures/images/actual-preview-udzo.dmg");
const IMAGE_NAME: &str = "fixture.dmg";
const OBSERVED_BACKGROUND: [u8; 3] = [23, 77, 129];
const ICON_EDGE: [u8; 3] = [103, 132, 129];

#[test]
fn renders_observed_finder_geometry_and_embedded_artwork() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join(IMAGE_NAME);
	let output = directory.path().join("actual.png");
	fs::write(&image, IMAGE).unwrap();
	let report = assert_success(invoke_image(&image, &output, &[]));
	assert!(
		report.starts_with("mode=actual\nwidth=160\nheight=100\n"),
		"{report}"
	);
	assert!(report.contains("finder_store=observed\n"), "{report}");
	assert!(report.contains("background=observed_png\n"), "{report}");
	assert!(report.contains("observed_positions=1\n"), "{report}");
	let (raster, metadata) = decode(&output);
	assert_eq!((metadata.width, metadata.height), (160, 100));
	assert_eq!(
		pixel(&raster, metadata.line_size, 2, 2),
		OBSERVED_BACKGROUND
	);
	assert_eq!(pixel(&raster, metadata.line_size, 30, 15), ICON_EDGE);
	assert_ne!(pixel(&raster, metadata.line_size, 95, 64), ICON_EDGE);

	let with_hidden = directory.path().join("hidden.png");
	let shown = assert_success(invoke_image(&image, &with_hidden, &["--show-hidden"]));
	assert!(shown.contains("observed_positions=2\n"), "{shown}");
	let (raster, metadata) = decode(&with_hidden);
	assert_eq!(pixel(&raster, metadata.line_size, 95, 64), ICON_EDGE);
	assert!(!invoke_image(&image, &output, &[]).status.success());
	assert_eq!(fs::read(&image).unwrap(), IMAGE);

	let raw = directory.path().join("fixture.raw");
	let export = Command::new(env!("CARGO_BIN_EXE_dmgez"))
		.arg("unwrap-raw")
		.arg(&image)
		.arg("--output")
		.arg(&raw)
		.output()
		.unwrap();
	assert!(export.status.success(), "{export:?}");
	let raw_png = directory.path().join("raw.png");
	assert_eq!(
		assert_success(invoke_image(&raw, &raw_png, &["--raw"])),
		report
	);
	assert_eq!(fs::read(&raw_png).unwrap(), fs::read(&output).unwrap());
}

#[test]
fn reports_an_exact_subfolder_without_a_store_as_fallback() {
	let directory = tempfile::tempdir().unwrap();
	let image = directory.path().join(IMAGE_NAME);
	let output = directory.path().join("actual.png");
	fs::write(&image, IMAGE).unwrap();
	let report = assert_success(invoke_image(&image, &output, &["--folder", "/Guide"]));
	assert!(report.contains("finder_store=absent\n"), "{report}");
	assert!(
		report.contains("finder_layout=generic_no_store\n"),
		"{report}"
	);
	assert!(report.contains("observed_positions=0\n"), "{report}");
	assert!(report.contains("fallback_positions=1\n"), "{report}");
	let missing = directory.path().join("missing.png");
	assert!(
		!invoke_image(&image, &missing, &["--folder", "/guide"])
			.status
			.success()
	);
	assert!(!missing.exists());
}
