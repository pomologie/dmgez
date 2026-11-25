//! Check name spelling, unit budgets and collisions across Finder store writes.

use std::collections::BTreeMap;

use dmgez_config::Project;
use dmgez_finder::{
	CatalogNames, Error, LayoutItem,
	edit::{EditableStore, IconLocationRecord},
	inspection::ReadLimits,
	write_store,
};
use ds_parser::{FieldKey, IconLocation, Value};

const COMPOSED: &str = "Café 🚀";
const STORED: &str = "Cafe\u{0301} 🚀";
const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Unicode"
output = "Unicode.dmg"
[finder.icon_view.positions]
"Café 🚀" = [17, 23]
"#;

#[test]
fn resolves_manifest_spelling_before_writing_the_stored_key() {
	let options = Project::parse(PROJECT).unwrap().finder.unwrap().options;
	let names = CatalogNames::new(|name| {
		assert_eq!(name, COMPOSED);
		Ok(STORED.to_owned())
	});
	let bytes = names
		.write_store(
			Some(&options),
			&[LayoutItem {
				name: COMPOSED,
				position: None,
			}],
		)
		.unwrap();
	let store = ds_parser::parse(&bytes).unwrap();
	let position = store
		.records
		.iter()
		.find(|record| record.field == FieldKey::Iloc)
		.unwrap();
	assert_eq!(position.name, STORED);
	assert_eq!(
		position.value,
		Value::IconLocation(IconLocation::new(17, 23))
	);

	let editable = EditableStore::open(&bytes, ReadLimits::default()).unwrap();
	assert_eq!(
		editable.positions().collect::<Vec<_>>(),
		[(STORED, IconLocationRecord::new(17, 23))]
	);
	let edited = editable
		.encode(&BTreeMap::from([(
			STORED.to_owned(),
			IconLocationRecord::new(27, 33),
		)]))
		.unwrap()
		.unwrap();
	let reopened = EditableStore::open(&edited, ReadLimits::default()).unwrap();
	assert_eq!(
		reopened.positions().collect::<Vec<_>>(),
		[(STORED, IconLocationRecord::new(27, 33))]
	);
}

#[test]
fn budgets_utf16_units_and_rejects_case_equivalent_position_keys() {
	let icons = Project::parse(PROJECT)
		.unwrap()
		.finder
		.unwrap()
		.options
		.icon_view
		.unwrap();
	let position = Some(icons.positions[COMPOSED]);
	let names: Vec<_> = (0..5)
		.map(|index| format!("{}{index}", "🚀".repeat(127)))
		.collect();
	let items: Vec<_> = names
		.iter()
		.map(|name| LayoutItem { name, position })
		.collect();
	let bytes = write_store(None, &items).unwrap();
	let store = ds_parser::parse(&bytes).unwrap();
	assert_eq!(
		store
			.records
			.iter()
			.filter(|record| record.field == FieldKey::Iloc)
			.count(),
		names.len()
	);
	let beyond_limit = "🚀".repeat(128);
	assert!(matches!(
		write_store(
			None,
			&[LayoutItem {
				name: &beyond_limit,
				position,
			}]
		),
		Err(Error::ItemName)
	));
	let items = ["Éclair", "éclair"].map(|name| LayoutItem { name, position });
	assert!(matches!(
		write_store(None, &items),
		Err(Error::DuplicateName(_))
	));
}
