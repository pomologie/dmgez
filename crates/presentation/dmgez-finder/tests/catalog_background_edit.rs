//! Qualified catalog background aliases for D-OPS-01.

use std::collections::BTreeMap;

use dmg_layout::{ExtraEntry, ExtrasTag};
use dmgez_config::Project;
use dmgez_finder::{
	AliasRecord, ImageBackgroundIdentity, LayoutItem, build_image_background_alias,
	edit::{EditableStore, IconLocationRecord, StoreEditError},
	inspection::{ReadBudget, ReadError, ReadLimits},
	write_store, write_store_with_background_alias, write_store_with_background_identity,
};
use ds_parser::{FieldKey, PropertyList, Value};

const ORIGINAL_ITEM: &str = "First Item";
const MOVED_ITEM: &str = "Moved Item";
const ICON_POSITION: IconLocationRecord = IconLocationRecord::new(31, 72);
const BACKGROUND_ALIAS_KEY: &str = "backgroundImageAlias";
const OPAQUE_PROPERTY: &str = "unknownSetting";
const OPAQUE_ALIAS_TAG: i16 = 97;
const LONG_PATH_DEPTH: usize = 12;
const LONG_COMPONENT_LENGTH: usize = 200;
const ORIGINAL: ImageBackgroundIdentity<'static> = ImageBackgroundIdentity {
	volume_name: "Original Installer",
	volume_created_hfs_seconds: 3_801_000_000,
	target_created_hfs_seconds: 3_801_000_123,
	parent_catalog_id: 16,
	target_catalog_id: 20,
	volume_relative_path: "/.background/desktop.png",
};
const REBOUND: ImageBackgroundIdentity<'static> = ImageBackgroundIdentity {
	volume_name: "Renamed Installe\u{301}r",
	parent_catalog_id: 35,
	target_catalog_id: 47,
	volume_relative_path: "/Assets/Artwork/Deskto\u{301}p.png",
	..ORIGINAL
};
const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Catalog Background"
output = "catalog.dmg"
[finder]
default_view = "list-view"
window_position = [82, 11]
window_size = [501, 299]
include_icon_view_settings = true
show_icon_preview = true
background = { kind = "image", source = "desktop.png" }
[finder.list_view]
columns = ["name", "size"]
sort_by = "size"
[finder.icon_view]
icon_size = 96
grid_offset = [2, 3]
[[contents]]
source = "file.txt"
destination = "/First Item"
position = [31, 72]
"#;

#[test]
fn admits_the_canonical_background_only_when_requested_and_retains_noop_bytes() {
	let source = store(ORIGINAL, ORIGINAL_ITEM);
	let limits = ReadLimits::default();
	assert!(matches!(
		EditableStore::open(&source, limits),
		Err(StoreEditError::UnsupportedRecord { .. })
	));
	let admitted = EditableStore::open_with_image_background(&source, limits).unwrap();
	assert_eq!(admitted.image_background(), Some(ORIGINAL));
	let positions = positions(ORIGINAL_ITEM);
	assert!(admitted.encode(&positions).unwrap().is_none());
	assert!(
		admitted
			.encode_with_image_background(&positions, ORIGINAL)
			.unwrap()
			.is_none()
	);
}

#[test]
fn replaces_the_alias_and_positions_without_changing_other_folder_properties() {
	let source = store(ORIGINAL, ORIGINAL_ITEM);
	let limits = ReadLimits::default();
	let admitted = EditableStore::open_with_image_background(&source, limits).unwrap();
	let positions = positions(MOVED_ITEM);
	let moved = admitted.encode(&positions).unwrap().unwrap();
	assert_eq!(moved, store(ORIGINAL, MOVED_ITEM));
	let rebound = admitted
		.encode_with_image_background(&positions, REBOUND)
		.unwrap()
		.unwrap();
	assert_eq!(rebound, store(REBOUND, MOVED_ITEM));
	let readback = EditableStore::open_with_image_background(&rebound, limits).unwrap();
	assert_eq!(readback.image_background(), Some(REBOUND));
	assert!(
		readback
			.encode_with_image_background(&positions, REBOUND)
			.unwrap()
			.is_none()
	);
}

#[test]
fn refuses_parseable_noncanonical_aliases_and_opaque_or_malformed_properties() {
	let project = Project::parse(PROJECT).unwrap();
	let options = &project.finder.as_ref().unwrap().options;
	let alias = build_image_background_alias(ORIGINAL).unwrap();
	let mut extras = alias
		.extras()
		.filter(|entry| entry.tag() != ExtrasTag::End)
		.cloned()
		.collect::<Vec<_>>();
	extras.push(ExtraEntry::new(ExtrasTag::Unknown(OPAQUE_ALIAS_TAG), vec![1]).unwrap());
	let alias = AliasRecord::new(alias.header, extras).unwrap();
	let source = write_store_with_background_alias(options, &[], &alias).unwrap();
	let limits = ReadLimits::default();
	assert!(matches!(
		EditableStore::open_with_image_background(&source, limits),
		Err(StoreEditError::UnqualifiedBackgroundAlias)
	));

	let source = store(ORIGINAL, ORIGINAL_ITEM);
	for (property, value) in [
		(OPAQUE_PROPERTY, PropertyList::Boolean(true)),
		(BACKGROUND_ALIAS_KEY, PropertyList::Data(vec![1])),
	] {
		let mut records = ds_parser::parse(&source).unwrap().records;
		let icon_view = records
			.iter_mut()
			.find(|record| record.field == FieldKey::icvp)
			.unwrap();
		let Value::IconViewProperties(view) = &mut icon_view.value else {
			panic!("expected an icon view property list");
		};
		view.unknown.insert(property.into(), value);
		assert!(matches!(
			EditableStore::open_with_image_background(&ds_parser::write(&records).unwrap(), limits),
			Err(StoreEditError::UnsupportedRecord { .. })
		));
	}
}

#[test]
fn retains_framing_and_budgets_and_rebinds_only_an_existing_alias() {
	let mut source = store(ORIGINAL, ORIGINAL_ITEM);
	let limits = ReadLimits::default();
	let shorter_limit = ReadLimits {
		store_bytes: source.len() - 1,
		..limits
	};
	assert!(matches!(
		EditableStore::open_with_image_background(&source, shorter_limit),
		Err(StoreEditError::Read(ReadError::Limit(
			ReadBudget::StoreBytes
		)))
	));
	source.push(1);
	assert!(matches!(
		EditableStore::open_with_image_background(&source, limits),
		Err(StoreEditError::UnqualifiedFraming)
	));
	source.pop();
	let admitted = EditableStore::open_with_image_background(&source, limits).unwrap();
	let component = format!("/{}", "a".repeat(LONG_COMPONENT_LENGTH));
	let prefix = component.repeat(LONG_PATH_DEPTH);
	let long_path = format!("{prefix}/background.png");
	let oversized = ImageBackgroundIdentity {
		volume_relative_path: &long_path,
		..REBOUND
	};
	assert!(matches!(
		admitted.encode_with_image_background(&positions(MOVED_ITEM), oversized),
		Err(StoreEditError::LeafBudget)
	));
	let default = write_store(None, &[]).unwrap();
	let admitted = EditableStore::open_with_image_background(&default, limits).unwrap();
	assert_eq!(admitted.image_background(), None);
	assert!(matches!(
		admitted.encode_with_image_background(&BTreeMap::new(), REBOUND),
		Err(StoreEditError::MissingBackgroundAlias)
	));
}

fn positions(item: &str) -> BTreeMap<String, IconLocationRecord> {
	BTreeMap::from([(item.to_owned(), ICON_POSITION)])
}

fn store(identity: ImageBackgroundIdentity<'_>, item: &str) -> Vec<u8> {
	let project = Project::parse(PROJECT).unwrap();
	let options = &project.finder.as_ref().unwrap().options;
	let items = [LayoutItem {
		name: item,
		position: project.contents[0].position,
	}];
	write_store_with_background_identity(options, &items, identity).unwrap()
}
