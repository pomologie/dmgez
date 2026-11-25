//! Actual-image raster and coverage evidence for D-GUI-02 and D-GUI-03.

#![cfg(feature = "actual")]

use std::io::Cursor;

use dmgez_config::Project;
use dmgez_finder::{
	ImageBackgroundIdentity, LayoutItem, build_image_background_alias,
	inspection::{ReadLimits, Record, RecordValue, Report, inspect},
	write_store, write_store_with_background_alias,
};
use dmgez_preview::{
	ItemVisibility,
	actual::{
		BackgroundAsset, BackgroundStatus, Entry, EntryKind, LayoutStatus, Store, StoreStatus,
	},
	raster::{ActualPreview, Error},
};

const BASIC: &str = "schema_version = 1\n[image]\nname = 'Actual'\noutput = 'test.dmg'\n";
const NAME: &str = "Guide.txt";

fn entry(name: &str) -> Entry {
	Entry {
		name: name.encode_utf16().collect(),
		kind: EntryKind::File,
		hidden: false,
		hide_extension: false,
	}
}

fn actual(entries: &[Entry], store: Store<'_>, asset: BackgroundAsset<'_>) -> ActualPreview {
	ActualPreview::from_image(entries, store, asset, ItemVisibility::Visible).unwrap()
}

fn pixel(preview: &ActualPreview, x: usize, y: usize) -> [u8; 3] {
	let start = (y * preview.size().0 as usize + x) * 3;
	preview.rgb()[start..start + 3].try_into().unwrap()
}

fn reported_style(options: &str) -> Report {
	let project = Project::parse(&format!("{BASIC}{options}")).unwrap();
	let options = &project.finder.as_ref().unwrap().options;
	let store = write_store(
		Some(options),
		&[LayoutItem {
			name: NAME,
			position: None,
		}],
	)
	.unwrap();
	inspect(&store, ReadLimits::default()).unwrap()
}

#[test]
fn renders_actual_store_dimensions_scroll_color_and_exact_stored_icon_center() {
	const BACKGROUND: [u8; 3] = [224, 234, 244];
	const ICON_EDGE: [u8; 3] = [103, 132, 129];
	let report = reported_style(
		r#"
[finder]
window_size = [160, 100]
background = { kind = 'color', value = '#e0eaf4' }
[finder.icon_view]
icon_size = 20
text_size = 14
scroll_position = [10, 4]
[finder.icon_view.positions]
"Guide.txt" = [40, 25]
"#,
	);
	let preview = actual(
		&[entry(NAME)],
		Store::Observed(&report),
		BackgroundAsset::Unavailable,
	);
	assert_eq!(preview.size(), (160, 100));
	assert_eq!(pixel(&preview, 0, 0), BACKGROUND);
	assert_eq!(pixel(&preview, 20, 11), ICON_EDGE);
	assert_eq!(pixel(&preview, 19, 11), BACKGROUND);
	let coverage = preview.coverage();
	assert_eq!(coverage.store, StoreStatus::Observed);
	assert_eq!(coverage.layout, LayoutStatus::ObservedIcon);
	assert_eq!(coverage.background, BackgroundStatus::ObservedColor);
	assert_eq!(
		(coverage.observed_positions, coverage.fallback_positions),
		(1, 0)
	);
	let mut encoded = Vec::new();
	preview.write_png(&mut encoded).unwrap();
	let reader = png::Decoder::new(Cursor::new(encoded)).read_info().unwrap();
	let tags = &reader.info().uncompressed_latin1_text;
	assert!(
		tags.iter()
			.any(|chunk| chunk.keyword == "Description"
				&& chunk.text == "dmgez actual-image preview")
	);
}

#[test]
fn matches_exact_utf16_without_discovering_stale_record_names() {
	let position = |name: &str| Record {
		name: name.to_owned(),
		field: *b"Iloc",
		data_type: *b"blob",
		value: RecordValue::IconPosition { x: 48, y: 48 },
	};
	// Case and canonical equivalence do not authorize reassignment of a stored location.
	let mut report = Report {
		records: vec![position("Café.txt"), position("Ghost.app")],
	};
	let entries = [
		entry("Cafe\u{301}.txt"),
		entry("Café.txt"),
		entry("café.txt"),
	];
	let rendered = actual(
		&entries,
		Store::Observed(&report),
		BackgroundAsset::Unavailable,
	);
	assert_eq!(rendered.coverage().layout, LayoutStatus::PartialIcon);
	assert_eq!(rendered.coverage().observed_positions, 1);
	assert_eq!(rendered.coverage().fallback_positions, 2);
	assert_eq!(pixel(&rendered, 16, 16), [103, 132, 129]);
	assert_eq!(pixel(&rendered, 126, 16), [103, 132, 129]);
	report.records.pop();
	let same = actual(
		&entries,
		Store::Observed(&report),
		BackgroundAsset::Unavailable,
	);
	assert_eq!(same.coverage(), rendered.coverage());
	assert_eq!(same.rgb(), rendered.rgb());
}

#[test]
fn separates_store_state_non_icon_views_and_hidden_entries() {
	let mut hidden = entry("Private");
	hidden.hidden = true;
	let entries = [entry(NAME), hidden];
	for (store, expected) in [
		(Store::Absent, StoreStatus::Absent),
		(Store::Unavailable, StoreStatus::Unavailable),
		(Store::Unsupported, StoreStatus::Unsupported),
		(Store::Malformed, StoreStatus::Malformed),
	] {
		let rendered = actual(&entries, store, BackgroundAsset::Unavailable);
		assert_eq!(rendered.coverage().store, expected);
		assert_eq!(rendered.coverage().layout, LayoutStatus::GenericNoStore);
		assert_eq!(rendered.coverage().fallback_positions, 1);
	}
	let report = reported_style("[finder]\ndefault_view = 'list-view'\n");
	let rendered = ActualPreview::from_image(
		&entries,
		Store::Observed(&report),
		BackgroundAsset::Unavailable,
		ItemVisibility::IncludeHidden,
	)
	.unwrap();
	assert_eq!(rendered.coverage().layout, LayoutStatus::GenericOtherView);
	assert_eq!(rendered.coverage().fallback_positions, 2);
	let duplicate = [entry(NAME), entry(NAME)];
	assert!(matches!(
		ActualPreview::from_image(
			&duplicate,
			Store::Absent,
			BackgroundAsset::Unavailable,
			ItemVisibility::Visible
		),
		Err(Error::Actual(dmgez_preview::actual::Error::DuplicateEntry))
	));
}

fn image_store() -> Report {
	let project = Project::parse(&format!(
		"{BASIC}[finder]\nbackground = {{ kind = 'image', source = 'plate.png' }}\n"
	))
	.unwrap();
	let alias = build_image_background_alias(ImageBackgroundIdentity {
		volume_name: "Actual",
		volume_created_hfs_seconds: 3_700_000_000,
		target_created_hfs_seconds: 3_700_000_001,
		parent_catalog_id: 2,
		target_catalog_id: 17,
		volume_relative_path: "/plate.png",
	})
	.unwrap();
	let options = &project.finder.as_ref().unwrap().options;
	let store = write_store_with_background_alias(options, &[], &alias).unwrap();
	inspect(&store, ReadLimits::default()).unwrap()
}

#[test]
fn distinguishes_observed_png_missing_invalid_and_other_image_backgrounds() {
	let report = image_store();
	let mut png = Vec::new();
	{
		let mut encoder = png::Encoder::new(&mut png, 1, 1);
		encoder.set_color(png::ColorType::Rgb);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header().unwrap();
		writer.write_image_data(&[11, 22, 33]).unwrap();
		writer.finish().unwrap();
	}
	let entries = [];
	let store = Store::Observed(&report);
	for (asset, status, expected_pixel) in [
		(
			BackgroundAsset::Encoded(&png),
			BackgroundStatus::ObservedPng,
			[11, 22, 33],
		),
		(
			BackgroundAsset::Unavailable,
			BackgroundStatus::UnavailableImage,
			[245, 249, 248],
		),
		(
			BackgroundAsset::Encoded(&png[..14]),
			BackgroundStatus::MalformedImage,
			[245, 249, 248],
		),
		(
			BackgroundAsset::Encoded(b"II*\0"),
			BackgroundStatus::UnsupportedImage,
			[245, 249, 248],
		),
	] {
		let preview = actual(&entries, store, asset);
		assert_eq!(preview.coverage().background, status);
		assert_eq!(pixel(&preview, 0, 0), expected_pixel);
	}
}
