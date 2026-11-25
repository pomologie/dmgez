//! Catalog-bound Finder store checks for D-OPS-01 and D-FINDER-03-T04.

use std::{
	collections::BTreeMap,
	io::Write,
	process::{Command, Stdio},
};

use dmgez_config::Project;
use dmgez_finder::{
	edit::{EditableStore, IconLocationRecord, StoreEditError},
	inspection::ReadLimits,
	write_store,
};
use ds_parser::{FieldKey, FourCC, IconLocation, Record, Value};
use plist::Dictionary;

const ROOT: &str = ".";
const ORIGINAL: &str = "First Item";
const RENAMED: &str = "Renamed Item";
const REMOVED: &str = "Removed Item";
const INSERTED: &str = "Inserted Item";
const UNSUPPORTED_TRAILING: [u8; 8] = [0xf0, 0xff, 0xf0, 0xff, 0xf0, 0xff, 0x03, 0x04];
const EXTENSION_FIELD: [u8; 4] = *b"extr";
const ORACLE: &str = include_str!("catalog_store_edit_oracle.py");
const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Catalog Edit"
output = "catalog.dmg"
[finder]
default_view = "list-view"
window_position = [82, 11]
window_size = [501, 299]
include_icon_view_settings = true
show_icon_preview = true
background = { kind = "color", value = "goldenrod" }
[finder.list_view]
columns = ["name", "date-added", "size"]
sort_by = "size"
[finder.icon_view]
icon_size = 96
grid_offset = [2, 3]
"#;

#[test]
fn admits_default_layout_and_keeps_noop_bytes() {
	let source = write_store(None, &[]).unwrap();
	let layout = EditableStore::open(&source, ReadLimits::default()).unwrap();
	assert_eq!(layout.positions().len(), 0);
	assert!(layout.encode(&BTreeMap::new()).unwrap().is_none());
}

#[test]
fn keeps_nondefault_folder_settings_and_icon_trailing_bytes_across_catalog_changes() {
	let (rebuilt, expected) = edited_store();
	assert_eq!(rebuilt, expected);
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_reader_reports_the_final_item_names_and_retained_views() {
	let (rebuilt, _) = edited_store();
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.stdin(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&rebuilt).unwrap();
	let output = oracle.wait_with_output().unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
}

fn edited_store() -> (Vec<u8>, Vec<u8>) {
	let project = Project::parse(PROJECT).unwrap();
	let source = write_store(Some(&project.finder.unwrap().options), &[]).unwrap();
	let mut records = ds_parser::parse(&source).unwrap().records;
	let folder_count = records.len();
	records.extend([
		Record::new(ORIGINAL, FieldKey::Iloc, IconLocation::new(31, 72)),
		Record::new(REMOVED, FieldKey::Iloc, IconLocation::new(5, 10)),
	]);
	let source = ds_parser::write(&records).unwrap();
	let layout = EditableStore::open(&source, ReadLimits::default()).unwrap();
	let mut positions: BTreeMap<String, _> = layout
		.positions()
		.map(|(name, record)| (name.to_owned(), record))
		.collect();
	assert!(layout.encode(&positions).unwrap().is_none());
	let moved = positions.remove(ORIGINAL).unwrap().with_position(120, 29);
	positions.remove(REMOVED).unwrap();
	positions.insert(RENAMED.to_owned(), moved);
	positions.insert(INSERTED.to_owned(), IconLocationRecord::new(111, 99));
	let rebuilt = layout.encode(&positions).unwrap().unwrap();
	let readback = EditableStore::open(&rebuilt, ReadLimits::default()).unwrap();
	assert_eq!(
		readback
			.positions()
			.map(|(name, record)| (name.to_owned(), record))
			.collect::<BTreeMap<_, _>>(),
		positions
	);
	records.truncate(folder_count);
	records.extend([
		Record::new(RENAMED, FieldKey::Iloc, IconLocation::new(120, 29)),
		Record::new(INSERTED, FieldKey::Iloc, IconLocation::new(111, 99)),
	]);
	(rebuilt, ds_parser::write(&records).unwrap())
}

#[test]
fn rejects_unqualified_fields_aliases_and_allocator_bytes() {
	let limits = ReadLimits::default();
	let mut source = write_store(None, &[]).unwrap();
	let mut records = ds_parser::parse(&source).unwrap().records;
	records.push(Record::new(
		ORIGINAL,
		FieldKey::Unknown(FourCC::from(EXTENSION_FIELD)),
		vec![1u8],
	));
	assert!(matches!(
		EditableStore::open(&ds_parser::write(&records).unwrap(), limits),
		Err(StoreEditError::UnsupportedRecord {
			field: EXTENSION_FIELD,
			..
		})
	));
	records.pop();
	records.push(Record::new(
		ORIGINAL,
		FieldKey::Iloc,
		IconLocation::with_trailing(31, 72, UNSUPPORTED_TRAILING),
	));
	assert!(matches!(
		EditableStore::open(&ds_parser::write(&records).unwrap(), limits),
		Err(StoreEditError::UnsupportedRecord { .. })
	));
	records.pop();
	for (property, value) in [
		("unqualifiedProperty", plist::Value::Boolean(true)),
		("backgroundImageAlias", plist::Value::Data(vec![1])),
	] {
		let mut with_unknown = records.clone();
		let record = with_unknown
			.iter_mut()
			.find(|record| record.name == ROOT && record.field == FieldKey::icvp)
			.unwrap();
		let dictionary = Dictionary::from_iter([(property.to_owned(), value)]);
		record.value = Value::PropertyList(plist::Value::Dictionary(dictionary));
		assert!(matches!(
			EditableStore::open(&ds_parser::write(&with_unknown).unwrap(), limits),
			Err(StoreEditError::UnsupportedRecord { .. })
		));
	}
	source.push(1);
	assert!(matches!(
		EditableStore::open(&source, limits),
		Err(StoreEditError::UnqualifiedFraming)
	));
}

#[test]
fn checks_output_name_and_page_budgets() {
	let source = write_store(None, &[]).unwrap();
	let layout = EditableStore::open(&source, ReadLimits::default()).unwrap();
	let position = IconLocationRecord::new(0, 100);
	let duplicate = BTreeMap::from([
		(ORIGINAL.to_owned(), position),
		(ORIGINAL.to_ascii_uppercase(), position),
	]);
	assert!(matches!(
		layout.encode(&duplicate),
		Err(StoreEditError::DuplicateName(_))
	));
	let too_many = (0..32)
		.map(|index| (format!("{index}-{}", "X".repeat(200)), position))
		.collect();
	assert!(matches!(
		layout.encode(&too_many),
		Err(StoreEditError::LeafBudget)
	));
}
