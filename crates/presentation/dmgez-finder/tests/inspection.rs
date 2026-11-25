//! Bounded Finder record checks for the Finder portion of D-READ-02.

use std::process::Command;

use dmgez_config::Project;
use dmgez_finder::{
	LayoutItem,
	inspection::{
		ReadBudget, ReadError, ReadLimits, Record, RecordValue, Report, ViewKind, WindowBounds,
		inspect,
	},
	write_store,
};
use ds_parser::{
	FieldKey, FourCC, IconLocation, Record as WireRecord, ValueTag,
	layout::{BuddyOffset, EncodedAddress},
};
use plist::{Dictionary, Value};

const PROJECT: &str = r##"
schema_version = 1
[image]
name = "Finder"
output = "finder.dmg"
[[contents]]
type = "directory"
destination = "/Example"
position = [150, 175]
[finder]
window_position = [75, -50]
window_size = [620, 270]
background = { kind = "color", value = "#246" }
show_status_bar = true
show_icon_preview = true
show_item_info = false
[finder.icon_view]
scroll_position = [2, 3]
"##;
const FOLDER: &str = ".";
const ITEM: &str = "Example";
const EXTRA_FIELD: [u8; 4] = *b"ext1";
const EXTRA_KEY: &str = "OpaquePreference";
const UNSUPPORTED_TYPE: [u8; 4] = *b"unkn";
const ALLOCATOR_OFFSET: usize = 8;
const ADDRESSES_OFFSET: usize = 8;
const TREE_DEPTH_OFFSET: usize = 4;
const LEAF_COUNT_OFFSET: usize = 4;
const SCROLL_X: &str = "scrollPositionX";
const SCROLL_Y: &str = "scrollPositionY";
const ORACLE: &str = include_str!("inspection_oracle.py");
const NATIVE: &[u8] = include_bytes!("fixtures/finder-macos.ds_store");

fn layout_store() -> Vec<u8> {
	let project = Project::parse(PROJECT).unwrap();
	let finder = project.finder.as_ref().unwrap();
	let items = [LayoutItem {
		name: ITEM,
		position: project.contents[0].position,
	}];
	write_store(Some(&finder.options), &items).unwrap()
}

fn record<'a>(report: &'a Report, name: &str, field: FieldKey) -> &'a Record {
	report
		.records
		.iter()
		.find(|record| record.name == name && record.field == field.fourcc().bytes())
		.unwrap()
}

fn check_layout(report: &Report) {
	assert_eq!(report.records.len(), 5);
	let RecordValue::Window(window) = &record(report, FOLDER, FieldKey::bwsp).value else {
		panic!("expected browser window settings");
	};
	assert_eq!(
		window.bounds,
		Some(WindowBounds {
			x: 75.0,
			y: -50.0,
			width: 620.0,
			height: 270.0
		})
	);
	assert_eq!(window.show_status_bar, Some(true));
	assert_eq!(window.show_toolbar, Some(false));
	assert_eq!(window.preview_pane_visible, Some(false));
	assert!(window.unreported_keys.is_empty());

	let RecordValue::IconView(icons) = &record(report, FOLDER, FieldKey::icvp).value else {
		panic!("expected icon-view settings");
	};
	assert_eq!(icons.arrangement.as_deref(), Some("none"));
	assert_eq!(icons.background_type, Some(1));
	assert_eq!(
		icons.background_color,
		Some([34.0 / 255.0, 68.0 / 255.0, 102.0 / 255.0])
	);
	assert_eq!(icons.background_alias, None);
	assert_eq!(icons.scroll_position, Some([2.0, 3.0]));
	assert_eq!(icons.show_icon_preview, Some(true));
	assert_eq!(icons.show_item_info, Some(false));
	assert!(icons.unreported_keys.is_empty());
	assert_eq!(
		record(report, FOLDER, FieldKey::vSrn).value,
		RecordValue::StoreVersion(1)
	);
	assert_eq!(
		record(report, FOLDER, FieldKey::icvl).value,
		RecordValue::View(ViewKind::Icon)
	);
	assert_eq!(
		record(report, ITEM, FieldKey::Iloc).value,
		RecordValue::IconPosition { x: 150, y: 175 }
	);
}

#[test]
fn reports_the_writer_settings_and_record_types() {
	let report = inspect(&layout_store(), ReadLimits::default()).unwrap();
	check_layout(&report);
	assert_eq!(
		record(&report, ITEM, FieldKey::Iloc).data_type,
		ValueTag::Blob.fourcc().bytes()
	);
}

#[test]
fn retains_only_opaque_identity_and_unknown_setting_names() {
	let unknown = FieldKey::Unknown(FourCC::from(EXTRA_FIELD));
	let window = Value::Dictionary(Dictionary::from_iter([(
		EXTRA_KEY.to_owned(),
		Value::Data(vec![5; 32]),
	)]));
	let bytes = ds_parser::write(&[
		WireRecord::new("Report-📋", unknown, vec![1, 5, 9]),
		WireRecord::new(FOLDER, FieldKey::bwsp, window),
	])
	.unwrap();
	let report = inspect(&bytes, ReadLimits::default()).unwrap();
	let opaque = record(&report, "Report-📋", unknown);
	assert_eq!(opaque.field, EXTRA_FIELD);
	assert_eq!(opaque.data_type, ValueTag::Blob.fourcc().bytes());
	assert_eq!(opaque.value, RecordValue::Opaque);
	let RecordValue::Window(window) = &record(&report, FOLDER, FieldKey::bwsp).value else {
		panic!("expected browser window settings");
	};
	assert_eq!(window.unreported_keys, [EXTRA_KEY]);
}

#[test]
fn applies_each_read_budget() {
	let bytes = layout_store();
	let defaults = ReadLimits::default();
	let cases = [
		(
			ReadLimits {
				store_bytes: bytes.len() - 1,
				..defaults
			},
			ReadBudget::StoreBytes,
		),
		(
			ReadLimits {
				records: 4,
				..defaults
			},
			ReadBudget::Records,
		),
		(
			ReadLimits {
				name_units: 1,
				..defaults
			},
			ReadBudget::NameUnits,
		),
		(
			ReadLimits {
				plist_objects: 0,
				..defaults
			},
			ReadBudget::PlistObjects,
		),
		(
			ReadLimits {
				plist_entries: 0,
				..defaults
			},
			ReadBudget::PlistEntries,
		),
	];
	for (limits, expected) in cases {
		assert!(
			matches!(inspect(&bytes, limits), Err(ReadError::Limit(actual)) if actual == expected)
		);
	}
}

#[test]
fn refuses_unsupported_trees_and_inconsistent_geometry() {
	let bytes = layout_store();
	let allocator = BuddyOffset::new(word_at(&bytes, ALLOCATOR_OFFSET))
		.file_index()
		.unwrap();
	let master = allocated_block(&bytes, allocator, 1);
	let root = allocated_block(&bytes, allocator, 2);

	let mut deep = bytes.clone();
	write_word(&mut deep, master + TREE_DEPTH_OFFSET, 1);
	assert!(matches!(
		inspect(&deep, ReadLimits::default()),
		Err(ReadError::UnsupportedTree)
	));

	let mut mismatched = bytes.clone();
	write_word(&mut mismatched, root + LEAF_COUNT_OFFSET, 3);
	assert!(matches!(
		inspect(&mismatched, ReadLimits::default()),
		Err(ReadError::RecordCount)
	));

	let mut repeated_allocator_offset = bytes.clone();
	write_word(&mut repeated_allocator_offset, ALLOCATOR_OFFSET, 0);
	assert!(matches!(
		inspect(&repeated_allocator_offset, ReadLimits::default()),
		Err(ReadError::Header)
	));
	assert!(matches!(
		inspect(&bytes[..bytes.len() - 1], ReadLimits::default()),
		Err(ReadError::Allocator)
	));
}

#[test]
fn rejects_unskippable_tags_and_duplicate_record_identities() {
	let locations = [
		WireRecord::new("Item", FieldKey::Iloc, IconLocation::new(1, 2)),
		WireRecord::new("item", FieldKey::Iloc, IconLocation::new(3, 4)),
	];
	let duplicates = ds_parser::write(&locations).unwrap();
	assert!(matches!(
		inspect(&duplicates, ReadLimits::default()),
		Err(ReadError::DuplicateRecord)
	));

	let mut bytes = ds_parser::write(&locations[..1]).unwrap();
	let field = FieldKey::Iloc.fourcc().bytes();
	let offset = bytes
		.windows(field.len())
		.position(|bytes| bytes == field)
		.unwrap()
		+ field.len();
	bytes[offset..offset + UNSUPPORTED_TYPE.len()].copy_from_slice(&UNSUPPORTED_TYPE);
	assert!(matches!(
		inspect(&bytes, ReadLimits::default()),
		Err(ReadError::UnsupportedType(UNSUPPORTED_TYPE))
	));
}

#[test]
fn checks_plist_shape_before_reporting_semantics() {
	let nested = Value::Dictionary(Dictionary::from_iter([(
		EXTRA_KEY.to_owned(),
		Value::Array(vec![Value::Boolean(true)]),
	)]));
	let bytes = ds_parser::write(&[WireRecord::new(FOLDER, FieldKey::bwsp, nested)]).unwrap();
	assert!(matches!(
		inspect(&bytes, ReadLimits::default()),
		Err(ReadError::PlistProfile)
	));

	let not_finite = Value::Dictionary(Dictionary::from_iter([
		(SCROLL_X.to_owned(), Value::Real(f64::NAN)),
		(SCROLL_Y.to_owned(), Value::Real(0.0)),
	]));
	let bytes = ds_parser::write(&[WireRecord::new(FOLDER, FieldKey::icvp, not_finite)]).unwrap();
	assert!(matches!(
		inspect(&bytes, ReadLimits::default()),
		Err(ReadError::SettingsNumber)
	));
}

#[test]
fn reports_exact_finder_authored_icon_centers() {
	let report = inspect(NATIVE, ReadLimits::default()).unwrap();
	assert_eq!(report.records.len(), 3);
	for (name, x, y) in [("bam", 104, 116), ("bar", 256, 235), ("baz", 454, 124)] {
		assert_eq!(
			record(&report, name, FieldKey::Iloc).value,
			RecordValue::IconPosition { x, y }
		);
	}
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn reports_a_store_built_by_the_independent_python_writer() {
	let result = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.output()
		.unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	check_layout(&inspect(&result.stdout, ReadLimits::default()).unwrap());
}

fn word_at(bytes: &[u8], offset: usize) -> u32 {
	u32::from_be_bytes(bytes[offset..offset + size_of::<u32>()].try_into().unwrap())
}

fn write_word(bytes: &mut [u8], offset: usize, word: u32) {
	bytes[offset..offset + size_of::<u32>()].copy_from_slice(&word.to_be_bytes());
}

fn allocated_block(bytes: &[u8], allocator: usize, block_id: usize) -> usize {
	let address = word_at(
		bytes,
		allocator + ADDRESSES_OFFSET + block_id * size_of::<u32>(),
	);
	EncodedAddress::new(address).file_range().unwrap().start
}
