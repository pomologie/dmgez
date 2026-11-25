use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmg_layout::{
	AliasHeader, AliasKind, AliasMetadata, AliasTarget, AliasVolume, CatalogNodeId, DiskType,
	FilesystemSignature, HfsTimestampSeconds,
};
use dmgez_config::{FinderOptions, Project};
use dmgez_finder::{
	AliasRecord, Error, validate_options, validate_options_for_image_background, write_store,
	write_store_with_background_alias,
};
use ds_parser::{FieldKey, PropertyList, Value, layout::BTreeMaster};

const IMAGE_OPTIONS: &str = "background = { kind = 'image', source = 'background.png' }";
const BACKGROUND_ALIAS_KEY: &str = "backgroundImageAlias";
const IMAGE_BACKGROUND_TYPE: i64 = 2;
const FOLDER_KEY: &str = ".";
const VOLUME_NAME: &str = "Example Installer";
const TARGET_NAME: &str = ".background.png";
const TARGET_PATH: &[u8] = b"/.background.png";
const PARENT_CNID: CatalogNodeId = CatalogNodeId(2);
const FILE_CNID: CatalogNodeId = CatalogNodeId(20);
const VOLUME_DATE: HfsTimestampSeconds = HfsTimestampSeconds(3_801_000_000);
const TARGET_DATE: HfsTimestampSeconds = HfsTimestampSeconds(3_801_000_123);
const ALIAS_VERSION: u16 = 2;
const ORACLE: &str = include_str!("background_alias_oracle.py");

fn options(background: &str) -> FinderOptions {
	let project = format!(
		"schema_version = 1\n[image]\nname = 'Example'\noutput = 'example.dmg'\n[finder]\n{background}\n"
	);
	Project::parse(&project).unwrap().finder.unwrap().options
}

fn alias(kind: AliasKind, target_path: &[u8]) -> AliasRecord {
	let volume = AliasVolume::new(
		VOLUME_NAME,
		VOLUME_DATE,
		FilesystemSignature::HIERARCHICAL_FILE_SYSTEM_PLUS,
		DiskType::Fixed,
	)
	.unwrap();
	let target = AliasTarget::new(kind, PARENT_CNID, TARGET_NAME, FILE_CNID, TARGET_DATE).unwrap();
	AliasRecord::from_metadata(
		AliasMetadata::builder()
			.header(AliasHeader::new(volume, target))
			.unicode_target_name(Some(TARGET_NAME.to_owned()))
			.unicode_volume_name(Some(VOLUME_NAME.to_owned()))
			.target_path(Some(target_path.to_vec()))
			.build(),
	)
	.unwrap()
}

#[test]
fn binds_the_image_alias_without_enabling_unresolved_backgrounds() {
	let options = options(IMAGE_OPTIONS);
	assert!(validate_options_for_image_background(&options).is_ok());
	assert!(matches!(
		validate_options(Some(&options)),
		Err(Error::ImageBackground)
	));
	assert!(matches!(
		write_store(Some(&options), &[]),
		Err(Error::ImageBackground)
	));

	let alias = alias(AliasKind::File, TARGET_PATH);
	let bytes = write_store_with_background_alias(&options, &[], &alias).unwrap();
	let store = ds_parser::parse(&bytes).unwrap();
	assert!(store.warnings.is_empty());
	let icon_record = store
		.records
		.iter()
		.find(|entry| entry.name == FOLDER_KEY && entry.field == FieldKey::icvp)
		.unwrap();
	let Value::IconViewProperties(icons) = &icon_record.value else {
		panic!("expected an icon view property list");
	};
	assert_eq!(icons.background_type, Some(IMAGE_BACKGROUND_TYPE));
	let Some(PropertyList::Data(encoded)) = icons.unknown.get(BACKGROUND_ALIAS_KEY) else {
		panic!("expected the image alias as binary property-list data");
	};
	assert_eq!(alias.to_bytes().unwrap(), *encoded);
	assert_eq!(
		AliasRecord::from_bytes(encoded).unwrap().header.version,
		ALIAS_VERSION
	);
}

#[test]
fn refuses_conflicting_settings_invalid_aliases_and_oversized_stores() {
	let image = options(IMAGE_OPTIONS);
	let mut valid = alias(AliasKind::File, TARGET_PATH);
	let color = options("background = { kind = 'color', value = '#246' }");
	assert!(matches!(
		validate_options_for_image_background(&color),
		Err(Error::ImageBackgroundNotSelected)
	));
	assert!(matches!(
		write_store_with_background_alias(&color, &[], &valid),
		Err(Error::ImageBackgroundNotSelected)
	));
	let excluded = options(&format!(
		"{IMAGE_OPTIONS}\ninclude_icon_view_settings = false"
	));
	assert!(matches!(
		validate_options_for_image_background(&excluded),
		Err(Error::IconViewExcluded)
	));
	assert!(matches!(
		write_store_with_background_alias(&excluded, &[], &valid),
		Err(Error::IconViewExcluded)
	));
	let folder = alias(AliasKind::Folder, TARGET_PATH);
	assert!(matches!(
		write_store_with_background_alias(&image, &[], &folder),
		Err(Error::ImageBackgroundTarget)
	));
	valid.header.version = 0;
	assert!(matches!(
		write_store_with_background_alias(&image, &[], &valid),
		Err(Error::ImageBackgroundAlias(_))
	));
	let oversized_path = vec![b'a'; BTreeMaster::PAGE_SIZE.as_usize()];
	let oversized = alias(AliasKind::File, &oversized_path);
	assert!(matches!(
		write_store_with_background_alias(&image, &[], &oversized),
		Err(Error::StoreBudget)
	));
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_decoder_recovers_the_on_volume_identity() {
	let options = options(IMAGE_OPTIONS);
	let alias = alias(AliasKind::File, TARGET_PATH);
	let bytes = write_store_with_background_alias(&options, &[], &alias).unwrap();
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&bytes).unwrap();
	assert!(oracle.wait().unwrap().success());
}
