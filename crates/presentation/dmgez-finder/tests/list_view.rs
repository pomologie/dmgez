//! List-view record checks for the layout portion of D-FINDER-02.

use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmgez_config::{Column, Project};
use dmgez_finder::inspection::{ListView, ReadLimits, RecordValue, Report, inspect};
use dmgez_finder::{Error, validate_options, write_store};
use ds_parser::{DSStore, FieldKey, FinderViewStyle, Value};
use plist::Dictionary;

const NATIVE: &[u8] = include_bytes!("fixtures/finder-list-14.7.2.ds_store");
const ORACLE: &str = include_str!("list_view_oracle.py");
const HEADER: &str = r#"
schema_version = 1
[image]
name = "Finder List"
output = "list.dmg"
[finder]
"#;
const LIST: &str = r#"
default_view = "list-view"
show_icon_preview = true
[finder.list_view]
columns = ["name", "date-added", "size"]
sort_by = "size"
icon_size = 32
text_size = 13
scroll_position = [2.5, 3.5]
use_relative_dates = false
calculate_all_sizes = true
[finder.list_view.column_widths]
name = 289
date-last-opened = 211
[finder.list_view.sort_directions]
size = "ascending"
date-last-opened = "ascending"
"#;
const NATIVE_SETTINGS: &str = r#"
default_view = "list-view"
show_icon_preview = true
[finder.list_view]
columns = ["name", "date-modified", "size", "kind"]
sort_by = "size"
icon_size = 16
text_size = 13
"#;
const COLUMNS: &str = "columns";
const IDENTIFIER: &str = "identifier";
const INDEX: &str = "index";
const WIDTH: &str = "width";
const VISIBLE: &str = "visible";
const ASCENDING: &str = "ascending";
const SCALAR_KEYS: [&str; 7] = [
	"sortColumn",
	"iconSize",
	"textSize",
	"calculateAllSizes",
	"useRelativeDates",
	"showIconPreview",
	"viewOptionsVersion",
];

fn project(settings: &str) -> Project {
	Project::parse(&format!("{HEADER}{settings}")).unwrap()
}

fn bytes(settings: &str) -> Vec<u8> {
	let project = project(settings);
	write_store(project.finder.as_ref().map(|finder| &finder.options), &[]).unwrap()
}

fn record(store: &DSStore, code: FieldKey) -> Option<&Value> {
	store
		.records
		.iter()
		.find(|record| record.name == "." && record.field == code)
		.map(|record| &record.value)
}

fn properties(store: &DSStore, code: FieldKey) -> Dictionary {
	match record(store, code).expect("list record should exist") {
		Value::ListViewProperties(value) => value.to_property_list(),
		Value::ListViewColumns(value) => value.to_property_list(),
		value => panic!("unexpected list record {value:?}"),
	}
	.into_dictionary()
	.unwrap()
}

fn ordered_column<'a>(options: &'a Dictionary, name: &str) -> &'a Dictionary {
	options[COLUMNS]
		.as_array()
		.unwrap()
		.iter()
		.find_map(|entry| {
			let column = entry.as_dictionary()?;
			(column[IDENTIFIER].as_string() == Some(name)).then_some(column)
		})
		.unwrap()
}

fn inspected_list(report: &Report, field: FieldKey) -> &ListView {
	let record = report
		.records
		.iter()
		.find(|record| record.name == "." && record.field == field.fourcc().bytes())
		.unwrap();
	match &record.value {
		RecordValue::ListView(view) => view,
		value => panic!("unexpected list record {value:?}"),
	}
}

#[test]
fn inspects_generated_list_records_with_the_same_column_semantics() {
	let report = inspect(&bytes(LIST), ReadLimits::default()).unwrap();
	let keyed = inspected_list(&report, FieldKey::lsvp);
	let ordered = inspected_list(&report, FieldKey::lsvC);
	assert_eq!(keyed, ordered);
	assert_eq!(ordered.sort_column.as_deref(), Some("size"));
	assert_eq!(ordered.scroll_position_x, Some(2.5));
	assert_eq!(ordered.scroll_position_y, Some(3.5));
	let columns = ordered.columns.as_deref().unwrap();
	assert_eq!(
		columns
			.iter()
			.filter(|column| column.visible)
			.map(|column| column.identifier.as_str())
			.collect::<Vec<_>>(),
		["name", "dateAdded", "size"]
	);
	assert_eq!(columns[0].width, 289);
	assert!(columns[2].ascending);
}

#[test]
fn inspects_native_sonoma_columns_in_each_record_order() {
	let report = inspect(NATIVE, ReadLimits::default()).unwrap();
	let keyed = inspected_list(&report, FieldKey::lsvp);
	let ordered = inspected_list(&report, FieldKey::lsvC);
	assert_eq!(keyed.sort_column.as_deref(), Some("size"));
	assert_eq!(keyed.sort_column, ordered.sort_column);
	assert_eq!(keyed.text_size, Some(13.0));
	let keyed = keyed.columns.as_deref().unwrap();
	let ordered = ordered.columns.as_deref().unwrap();
	assert_eq!(keyed.len(), 9);
	assert_eq!(ordered.len(), 14);
	assert_eq!(keyed[1].identifier, "dateModified");
	assert_eq!(ordered[1].identifier, "ubiquity");
	assert!(!ordered[1].visible);
	assert_eq!(ordered[1].width, 35);
}

#[test]
fn writes_the_list_default_coordinate_fields_and_both_column_forms() {
	let first = bytes(LIST);
	assert_eq!(first, bytes(LIST));
	let store = ds_parser::parse(&first).unwrap();
	assert!(store.warnings.is_empty(), "{:?}", store.warnings);
	assert_eq!(
		record(&store, FieldKey::icvl),
		Some(&Value::Type(FinderViewStyle::List.fourcc()))
	);
	assert!(record(&store, FieldKey::icvp).is_none());
	let keyed = properties(&store, FieldKey::lsvp);
	let ordered = properties(&store, FieldKey::lsvC);
	for key in SCALAR_KEYS {
		assert_eq!(keyed[key], ordered[key]);
	}
	assert_eq!(keyed["sortColumn"].as_string(), Some("size"));
	assert_eq!(keyed["iconSize"].as_real(), Some(32.0));
	assert_eq!(keyed["textSize"].as_real(), Some(13.0));
	assert_eq!(keyed["scrollPositionX"].as_real(), Some(2.5));
	assert_eq!(keyed["scrollPositionY"].as_real(), Some(3.5));
	assert_eq!(keyed["showIconPreview"].as_boolean(), Some(true));
	assert_eq!(keyed["calculateAllSizes"].as_boolean(), Some(true));
	assert_eq!(keyed["useRelativeDates"].as_boolean(), Some(false));
	let columns = keyed[COLUMNS].as_dictionary().unwrap();
	let ordered_columns = ordered[COLUMNS].as_array().unwrap();
	assert_eq!(columns.len(), 10);
	assert_eq!(ordered_columns.len(), 10);
	for (index, name) in ["name", "dateAdded", "size"].into_iter().enumerate() {
		assert_eq!(
			ordered_columns[index].as_dictionary().unwrap()[IDENTIFIER].as_string(),
			Some(name)
		);
		let column = columns[name].as_dictionary().unwrap();
		assert_eq!(column[VISIBLE].as_boolean(), Some(true));
		assert_eq!(column[INDEX].as_unsigned_integer(), Some(index as u64));
	}
	assert_eq!(
		columns["name"].as_dictionary().unwrap()[WIDTH].as_signed_integer(),
		Some(289)
	);
	assert_eq!(
		columns["size"].as_dictionary().unwrap()[ASCENDING].as_boolean(),
		Some(true)
	);
	let hidden = ordered_column(&ordered, "dateLastOpened");
	assert_eq!(hidden[VISIBLE].as_boolean(), Some(false));
	assert_eq!(hidden[ASCENDING].as_boolean(), Some(true));
	assert_eq!(hidden[WIDTH].as_signed_integer(), Some(211));
}

#[test]
fn preserves_native_column_semantics_without_emitting_extra_cloud_columns() {
	let native = ds_parser::parse(NATIVE).unwrap();
	let ours = ds_parser::parse(&bytes(NATIVE_SETTINGS)).unwrap();
	let native = properties(&native, FieldKey::lsvC);
	let ours = properties(&ours, FieldKey::lsvC);
	for key in SCALAR_KEYS {
		assert_eq!(native[key], ours[key], "{key}");
	}
	for name in [
		"name",
		"dateModified",
		"dateCreated",
		"dateAdded",
		"dateLastOpened",
		"size",
		"kind",
		"label",
		"version",
		"comments",
	] {
		for key in [IDENTIFIER, WIDTH, ASCENDING, VISIBLE] {
			assert_eq!(
				ordered_column(&native, name)[key],
				ordered_column(&ours, name)[key],
				"{name}.{key}"
			);
		}
	}
}

#[test]
fn emits_both_layouts_when_requested_and_rejects_contradictions() {
	let both = ds_parser::parse(&bytes("include_list_view_settings = true")).unwrap();
	assert_eq!(
		record(&both, FieldKey::icvl),
		Some(&Value::Type(FinderViewStyle::Icon.fourcc()))
	);
	for field in [FieldKey::icvp, FieldKey::lsvp, FieldKey::lsvC] {
		assert!(record(&both, field).is_some());
	}
	let invalid = project("include_list_view_settings = false\n[finder.list_view]\nicon_size = 32");
	assert!(matches!(
		validate_options(invalid.finder.as_ref().map(|finder| &finder.options)),
		Err(Error::ListViewExcluded)
	));
	let invalid = project(
		"default_view = 'list-view'\ninclude_icon_view_settings = false\n[finder.icon_view]\nicon_size = 64",
	);
	assert!(matches!(
		validate_options(invalid.finder.as_ref().map(|finder| &finder.options)),
		Err(Error::IconViewExcluded)
	));
	let invalid = project("[finder.list_view.column_widths]\nname = 234.5");
	assert!(matches!(
		validate_options(invalid.finder.as_ref().map(|finder| &finder.options)),
		Err(Error::ListColumnWidth(Column::Name))
	));
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_reader_reports_the_generated_list_records() {
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle
		.stdin
		.take()
		.unwrap()
		.write_all(&bytes(LIST))
		.unwrap();
	assert!(oracle.wait().unwrap().success());
}
