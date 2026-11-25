//! Stored image-background identities and diagnostics for D-READ-02.

use dmg_layout::{
	AliasHeader, AliasHeaderV3, AliasKind, AliasRecord, AliasRecordV3, AliasTarget, AliasVolume,
	CatalogNodeId, DiskType, ExtraEntry, FilesystemSignature, HfsTimestamp, HfsTimestampSeconds,
};
use dmgez_finder::inspection::{
	BackgroundAlias, BackgroundAliasName, HfsCreationTime, ReadLimits, RecordValue, inspect,
};
use ds_parser::{FieldKey, Record};
use plist::{Dictionary, Value};

const ROOT: &str = ".";
const BACKGROUND_TYPE: &str = "backgroundType";
const BACKGROUND_IMAGE_ALIAS: &str = "backgroundImageAlias";
const IMAGE_BACKGROUND: i64 = 2;
const LEGACY_VOLUME_NAME: &[u8] = b"Caf\x8e";
const VOLUME_NAME: &str = "Café";
const TARGET_NAME: &str = "background.png";
const TARGET_PATH: &[u8] = b"/.background/background-\xff.png";
const PARENT_ID: CatalogNodeId = CatalogNodeId(20);
const TARGET_ID: CatalogNodeId = CatalogNodeId(21);
const VOLUME_DATE: HfsTimestampSeconds = HfsTimestampSeconds(3_801_000_000);
const TARGET_DATE: HfsTimestampSeconds = HfsTimestampSeconds(3_801_000_123);

fn alias_header(kind: AliasKind) -> AliasHeader {
	let volume = AliasVolume::new(
		LEGACY_VOLUME_NAME,
		VOLUME_DATE,
		FilesystemSignature::HIERARCHICAL_FILE_SYSTEM_PLUS,
		DiskType::Ejectable,
	)
	.unwrap();
	let target = AliasTarget::new(kind, PARENT_ID, TARGET_NAME, TARGET_ID, TARGET_DATE).unwrap();
	AliasHeader::new(volume, target)
}

fn inspect_background(alias: Option<Value>) -> BackgroundAlias {
	let mut props =
		Dictionary::from_iter([(BACKGROUND_TYPE.to_owned(), Value::from(IMAGE_BACKGROUND))]);
	if let Some(alias) = alias {
		props.insert(BACKGROUND_IMAGE_ALIAS.to_owned(), alias);
	}
	let store =
		ds_parser::write(&[Record::new(ROOT, FieldKey::icvp, Value::Dictionary(props))]).unwrap();
	let record = inspect(&store, ReadLimits::default())
		.unwrap()
		.records
		.pop()
		.unwrap();
	let RecordValue::IconView(icons) = record.value else {
		panic!("expected icon-view settings");
	};
	assert_eq!(icons.background_type, Some(IMAGE_BACKGROUND));
	assert!(icons.unreported_keys.is_empty());
	icons.background_alias.unwrap()
}

#[test]
fn reports_version_two_catalog_identity_and_raw_path() {
	let alias = AliasRecord::new(
		alias_header(AliasKind::File),
		vec![
			ExtraEntry::unicode_volume_name(VOLUME_NAME).unwrap(),
			ExtraEntry::portable_operating_system_interface_path(TARGET_PATH).unwrap(),
		],
	)
	.unwrap();
	let BackgroundAlias::File(target) =
		inspect_background(Some(Value::Data(alias.to_bytes().unwrap())))
	else {
		panic!("expected a file identity");
	};
	assert_eq!(target.version, 2);
	assert_eq!(
		target.volume_name,
		Some(BackgroundAliasName::Unicode(VOLUME_NAME.to_owned()))
	);
	assert_eq!(
		target.target_name,
		Some(BackgroundAliasName::Legacy(TARGET_NAME.as_bytes().to_vec()))
	);
	assert_eq!(target.target_path.as_deref(), Some(TARGET_PATH));
	assert_eq!(target.parent_catalog_id, PARENT_ID.raw());
	assert_eq!(target.target_catalog_id, TARGET_ID.raw());
	assert_eq!(
		target.volume_created,
		HfsCreationTime::Seconds(VOLUME_DATE.raw())
	);
	assert_eq!(
		target.target_created,
		HfsCreationTime::Seconds(TARGET_DATE.raw())
	);
}

#[test]
fn preserves_version_three_timestamp_fractions_and_optional_names() {
	const VOLUME_FRACTION: u64 = 7;
	const TARGET_FRACTION: u64 = 123;
	const PATH: &[u8] =
		b"/.background/synthetic-version-three-background-image-inspection-fixture-with-path.png";
	let volume_created =
		u64::from(VOLUME_DATE.raw()) * HfsTimestamp::UNITS_PER_SECOND + VOLUME_FRACTION;
	let target_created =
		u64::from(TARGET_DATE.raw()) * HfsTimestamp::UNITS_PER_SECOND + TARGET_FRACTION;
	let alias = AliasRecordV3::new(
		AliasHeaderV3::new(
			AliasKind::File,
			HfsTimestamp(volume_created),
			*b"HFS+",
			DiskType::Ejectable,
			PARENT_ID,
			TARGET_ID,
			HfsTimestamp(target_created),
		),
		vec![
			ExtraEntry::unicode_target_name(TARGET_NAME).unwrap(),
			ExtraEntry::portable_operating_system_interface_path(PATH).unwrap(),
		],
	)
	.unwrap();
	let BackgroundAlias::File(target) =
		inspect_background(Some(Value::Data(alias.to_bytes().unwrap())))
	else {
		panic!("expected a file identity");
	};
	assert_eq!(target.version, 3);
	assert_eq!(target.volume_name, None);
	assert_eq!(
		target.target_name,
		Some(BackgroundAliasName::Unicode(TARGET_NAME.to_owned()))
	);
	assert_eq!(target.target_path.as_deref(), Some(PATH));
	assert_eq!(target.parent_catalog_id, PARENT_ID.raw());
	assert_eq!(target.target_catalog_id, TARGET_ID.raw());
	assert_eq!(
		target.volume_created,
		HfsCreationTime::FixedPoint(volume_created)
	);
	assert_eq!(
		target.target_created,
		HfsCreationTime::FixedPoint(target_created)
	);
}

#[test]
fn reports_missing_malformed_and_unsupported_aliases_separately() {
	const UNSUPPORTED_VERSION: u16 = 7;
	const VERSION_OFFSET: usize = size_of::<[u8; 4]>() + size_of::<u16>();
	let mut unsupported = AliasRecord::new(alias_header(AliasKind::File), vec![])
		.unwrap()
		.to_bytes()
		.unwrap();
	unsupported[VERSION_OFFSET..][..size_of::<u16>()]
		.copy_from_slice(&UNSUPPORTED_VERSION.to_be_bytes());
	let folder = AliasRecord::new(alias_header(AliasKind::Folder), vec![])
		.unwrap()
		.to_bytes()
		.unwrap();
	let duplicate = AliasRecord::new(
		alias_header(AliasKind::File),
		vec![
			ExtraEntry::unicode_volume_name(VOLUME_NAME).unwrap(),
			ExtraEntry::unicode_volume_name("Another stored name").unwrap(),
		],
	)
	.unwrap()
	.to_bytes()
	.unwrap();
	for (property, expected) in [
		(None, BackgroundAlias::Missing),
		(Some(Value::Boolean(true)), BackgroundAlias::Malformed),
		(Some(Value::Data(vec![0])), BackgroundAlias::Malformed),
		(Some(Value::Data(duplicate)), BackgroundAlias::Malformed),
		(
			Some(Value::Data(unsupported)),
			BackgroundAlias::UnsupportedVersion(UNSUPPORTED_VERSION),
		),
		(
			Some(Value::Data(folder)),
			BackgroundAlias::UnsupportedTarget,
		),
	] {
		assert_eq!(inspect_background(property), expected);
	}
}
