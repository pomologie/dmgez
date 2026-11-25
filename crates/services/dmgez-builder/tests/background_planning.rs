//! Generated Finder file planning and captured background input validation.

#![cfg(all(feature = "finder-write", any(unix, windows)))]

use std::{fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	manifest,
	planning::{self, PreparedBuild},
	preflight::{
		Availability, BuildCapabilities, BuildFeature, FinderPlan, OutputIntent, PreflightError,
		PreflightOptions, ReleaseIntent, preflight,
	},
	presentation::{Error, ImageBackgroundIdentity},
};
use dmgez_config::Project;
use dmgez_udif::writer::Cancellation;

const PROJECT: &str = "schema_version = 1\n[image]\nname = 'Background test'\nformat = 'udro'\n";
const ROOT_IMAGE: &str = "\n[finder]\nbackground = { kind = 'image', source = 'root.png' }\n";
const RETINA_IMAGE: &str =
	"\n[finder]\nbackground = { kind = 'image', source = 'root.png', retina = 'root-2x.png' }\n";
const CONTENTS: &str = r#"
[[contents]]
type = "directory"
destination = "/Docs"
[[contents]]
type = "directory"
destination = "/Help"
[[contents]]
type = "directory"
destination = "/Text"
"#;
const FOLDERS: &str = r#"
[[finder.folders]]
path = "/Docs"
[[finder.folders]]
path = "/Help"
background = { kind = "image", source = "help.png" }
[[finder.folders]]
path = "/Text"
background = { kind = "none" }
"#;

fn options(image_background: Availability) -> PreflightOptions {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.finder_layout = Availability::Available;
	capabilities.finder_image_background = image_background;
	PreflightOptions {
		capabilities,
		output: OutputIntent::Path,
		release: ReleaseIntent::Unsigned,
		host_threads: NonZeroUsize::MIN,
	}
}

fn prepare(text: &str, base: &Path) -> Result<PreparedBuild, planning::Error> {
	let project = Project::parse(text).unwrap();
	planning::prepare(
		&project,
		base,
		options(Availability::Available),
		Cancellation::default(),
	)
}

fn square_png(value: u8, side: u32) -> Vec<u8> {
	let mut bytes = Vec::new();
	let mut encoder = png::Encoder::new(&mut bytes, side, side);
	encoder.set_color(png::ColorType::Grayscale);
	let mut writer = encoder.write_header().unwrap();
	writer
		.write_image_data(&vec![value; usize::try_from(side * side).unwrap()])
		.unwrap();
	writer.finish().unwrap();
	bytes
}

fn png(value: u8) -> Vec<u8> {
	square_png(value, 1)
}

#[test]
fn reserves_one_inherited_asset_and_distinct_override_before_store_binding() {
	let base = tempfile::tempdir().unwrap();
	let root = png(17);
	let help = png(42);
	fs::write(base.path().join("root.png"), &root).unwrap();
	fs::write(base.path().join("help.png"), &help).unwrap();
	let plan = prepare(
		&format!("{PROJECT}output = 'output.dmg'\n{CONTENTS}{ROOT_IMAGE}{FOLDERS}"),
		base.path(),
	)
	.unwrap();
	assert_eq!(
		plan.finder_directories()
			.iter()
			.map(|path| path.as_str())
			.collect::<Vec<_>>(),
		["/.background"],
	);
	assert_eq!(plan.finder_asset_count(), 6);
	assert_eq!(plan.finder_assets()[0].bytes, root);
	assert_eq!(plan.finder_assets()[1].bytes, help);
	assert_eq!(plan.finder_assets()[2].path.as_str(), "/Text/.DS_Store");
	let stores = plan.deferred_finder_stores();
	assert_eq!(stores.len(), 3);
	assert_eq!(
		(
			stores[0].path().as_str(),
			stores[0].background_path().as_str()
		),
		("/.DS_Store", plan.finder_assets()[0].path.as_str())
	);
	assert_eq!(
		(
			stores[1].path().as_str(),
			stores[1].background_path().as_str()
		),
		("/Docs/.DS_Store", stores[0].background_path().as_str())
	);
	assert_eq!(
		(
			stores[2].path().as_str(),
			stores[2].background_path().as_str()
		),
		("/Help/.DS_Store", plan.finder_assets()[1].path.as_str())
	);
	assert!(plan.deferred_finder_metadata_bytes() > 0);
	assert!(matches!(
		stores[0].render(ImageBackgroundIdentity {
			volume_name: "Background test",
			volume_created_hfs_seconds: 2_082_844_801,
			target_created_hfs_seconds: 2_082_844_801,
			parent_catalog_id: 16,
			target_catalog_id: 17,
			volume_relative_path: stores[2].background_path().as_str(),
		}),
		Err(Error::BackgroundTargetMismatch)
	));
}

#[test]
fn preflights_both_background_forms_before_source_access() {
	for background in [ROOT_IMAGE, RETINA_IMAGE] {
		let project =
			Project::parse(&format!("{PROJECT}output = 'output.dmg'\n{background}")).unwrap();
		let missing = Path::new("missing relative working directory");
		let result = planning::prepare(
			&project,
			missing,
			options(Availability::NotCompiled),
			Cancellation::default(),
		);
		assert!(matches!(
			result,
			Err(planning::Error::Preflight(
				PreflightError::FeatureNotCompiled(BuildFeature::FinderImageBackground)
			))
		));
		assert_eq!(
			preflight(&project, options(Availability::Available))
				.unwrap()
				.finder(),
			FinderPlan::IconView,
		);
	}
}

#[test]
fn distinguishes_retina_pairs_and_single_sources_and_binds_their_final_paths() {
	const RETINA_FOLDERS: &str = r#"
[[finder.folders]]
path = "/Docs"
[[finder.folders]]
path = "/Help"
background = { kind = "image", source = "root.png", retina = "help-2x.png" }
[[finder.folders]]
path = "/Text"
background = { kind = "image", source = "root.png" }
"#;
	let base = tempfile::tempdir().unwrap();
	let original = png(17);
	fs::write(base.path().join("root.png"), &original).unwrap();
	fs::write(base.path().join("root-2x.png"), square_png(42, 2)).unwrap();
	fs::write(base.path().join("help-2x.png"), square_png(81, 2)).unwrap();
	let plan = prepare(
		&format!("{PROJECT}output = 'output.dmg'\n{CONTENTS}{RETINA_IMAGE}{RETINA_FOLDERS}"),
		base.path(),
	)
	.unwrap();
	let assets = plan.finder_assets();
	assert_eq!(
		assets
			.iter()
			.map(|asset| asset.path.as_str())
			.collect::<Vec<_>>(),
		[
			"/.background/background-0.tiff",
			"/.background/background-1.tiff",
			"/.background/background-2.png",
		],
	);
	assert_eq!(assets[2].bytes, original);
	assert_ne!(assets[0].bytes, assets[1].bytes);
	let stores = plan.deferred_finder_stores();
	for (store, asset) in stores
		.iter()
		.zip([&assets[0], &assets[0], &assets[1], &assets[2]])
	{
		assert_eq!(store.background_path(), &asset.path);
	}
	assert_eq!(stores.len(), 4);

	// A prebuilt TIFF need not carry a TIFF extension on the host.
	fs::write(base.path().join("root.png"), &assets[0].bytes).unwrap();
	let prebuilt = prepare(
		&format!("{PROJECT}output = 'output.dmg'\n{ROOT_IMAGE}"),
		base.path(),
	)
	.unwrap();
	assert_eq!(prebuilt.finder_assets(), &assets[..1]);
	assert_eq!(
		prebuilt.deferred_finder_stores()[0].background_path(),
		&assets[0].path,
	);
}

#[test]
fn rejects_an_output_alias_of_the_retina_source() {
	let base = tempfile::tempdir().unwrap();
	fs::write(base.path().join("root.png"), png(17)).unwrap();
	fs::write(base.path().join("root-2x.png"), square_png(42, 2)).unwrap();
	let alias = format!("{PROJECT}output = 'root-2x.png'\n{RETINA_IMAGE}");
	assert!(matches!(
		prepare(&alias, base.path()),
		Err(planning::Error::Presentation(Error::BackgroundSource(
			manifest::Error::OutputSourceAlias { .. }
		)))
	));
}

#[test]
fn generated_retina_tiff_must_fit_the_build_metadata_limit() {
	let base = tempfile::tempdir().unwrap();
	let standard = square_png(17, 256);
	let retina = square_png(42, 512);
	fs::write(base.path().join("root.png"), &standard).unwrap();
	fs::write(base.path().join("root-2x.png"), &retina).unwrap();
	let project = format!("{PROJECT}output = 'output.dmg'\n{RETINA_IMAGE}");
	let plan = prepare(&project, base.path()).unwrap();
	assert!(plan.finder_assets()[0].bytes.len() > standard.len() + retina.len());
	let limited = format!("{project}\n[performance]\nmax_metadata_bytes = '96KiB'\n");
	assert!(matches!(
		prepare(&limited, base.path()),
		Err(planning::Error::Presentation(Error::MetadataBudget))
	));
}

#[test]
fn rejects_invalid_png_and_aliased_output_and_accounts_reserved_entries() {
	let base = tempfile::tempdir().unwrap();
	let source = base.path().join("root.png");
	let project = format!("{PROJECT}output = 'output.dmg'\n{ROOT_IMAGE}");
	fs::write(&source, b"invalid PNG").unwrap();
	assert!(matches!(
		prepare(&project, base.path()),
		Err(planning::Error::Presentation(Error::BackgroundPng(_)))
	));
	const IMAGE_DATA: &[u8] = b"IDAT";
	let mut corrupt = png(17);
	let compressed = corrupt
		.windows(IMAGE_DATA.len())
		.position(|bytes| bytes == IMAGE_DATA)
		.unwrap()
		+ IMAGE_DATA.len();
	corrupt[compressed] ^= 0x80;
	fs::write(&source, corrupt).unwrap();
	assert!(matches!(
		prepare(&project, base.path()),
		Err(planning::Error::Presentation(Error::BackgroundPng(_)))
	));
	fs::write(&source, png(17)).unwrap();
	let limits = format!("{project}\n[performance]\nmax_entries = 2\n");
	assert!(matches!(
		prepare(&limits, base.path()),
		Err(planning::Error::Presentation(Error::EntryBudget))
	));
	let alias = format!("{PROJECT}output = 'root.png'\n{ROOT_IMAGE}");
	assert!(matches!(
		prepare(&alias, base.path()),
		Err(planning::Error::Presentation(Error::BackgroundSource(
			manifest::Error::OutputSourceAlias { .. }
		)))
	));
}
