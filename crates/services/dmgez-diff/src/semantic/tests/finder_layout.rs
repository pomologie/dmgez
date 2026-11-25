use dmgez_finder::{
	STORE_NAME,
	inspection::{ReadBudget, ReadError, ReadLimits},
	write_store,
};

use super::{
	ChangeKind, Error, Field, Limits, Outcome, TestNode, TestTree, compare, view,
	view_with_metadata,
};
use crate::semantic::{
	ContentComparison, FinderLayoutError, Options, Uncompared, compare_with_finder_layout,
	compare_with_options,
};
use dmgez_vfs::volume::{FINDER_INFO_BYTES, FinderInfo, VolumeView};

const STORE_VERSION_TAG: [u8; 4] = *b"vSrn";
const LONG_TYPE_TAG: [u8; 4] = *b"long";
const ICON_VIEW_BLOB_HEADER: [u8; 8] = *b"icvpblob";
const ONE_BYTE_PLIST_INTEGER: u8 = 0x10;
const DEFAULT_BACKGROUND_TYPE: u8 = 0;
const IMAGE_BACKGROUND_TYPE: u8 = 2;
const OPAQUE_TAG: [u8; 4] = *b"test";
const FOLDER_RECORD_NAME: &str = ".";
const FILE_ID: u32 = 2;

#[test]
fn compares_reported_records_independently_of_unused_store_bytes() {
	let store = write_store(None, &[]).unwrap();
	let mut shifted = store.clone();
	shifted.push(1);
	let mut left = store_view(&store);
	let mut right = store_view(&shifted);
	let baseline = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(baseline.changes[0].fields, [Field::DataFork]);
	assert_eq!(baseline.policy.contents, ContentComparison::ExactBytes);
	assert!(baseline.policy.finder_layout.is_none());

	let report =
		compare_with_finder_layout(&mut left, &mut right, Limits::default(), true, || false)
			.unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert_eq!(
		report.policy.contents,
		ContentComparison::ExactBytesWithFinderLayout
	);
	assert_eq!(report.compared_bytes, shifted.len() as u64);
	assert!(report.policy.xattrs.is_some());
	assert!(
		report
			.policy
			.excluded
			.contains(&Uncompared::PerObjectFinderInfo)
	);
	assert_eq!(report.policy.finder_layout.unwrap().store_name, STORE_NAME);
}

#[test]
fn independently_composes_both_finder_scopes_and_stored_attributes() {
	let root_finder_info = FinderInfo::Present([0x5a; FINDER_INFO_BYTES]);
	let mut left = view([]);
	let mut right = view_with_metadata([], true, root_finder_info);
	for xattrs in [false, true] {
		let report = compare_with_options(
			&mut left,
			&mut right,
			Limits::default(),
			Options {
				xattrs,
				finder_layout: true,
				finder_info: true,
			},
			|| false,
		)
		.unwrap();
		assert_eq!(report.changes[0].fields, [Field::FinderInfo]);
		assert!(report.policy.finder_layout.is_some());
		assert!(report.policy.finder_info.is_some());
		assert_eq!(report.policy.xattrs.is_some(), xattrs);
		assert!(report.policy.compared.contains(&Field::FinderLayout));
		assert!(report.policy.compared.contains(&Field::FinderInfo));
		assert_eq!(
			report.policy.compared.contains(&Field::ExtendedAttributes),
			xattrs
		);
		for field in [
			Uncompared::FinderLayout,
			Uncompared::PerObjectFinderInfo,
			Uncompared::FinderMetadata,
		] {
			assert!(!report.policy.excluded.contains(&field));
		}
	}
}

#[test]
fn records_reported_field_changes_without_repeating_raw_data_differences() {
	let store = write_store(None, &[]).unwrap();
	let mut changed = store.clone();
	let version = version_record(&mut changed);
	version[STORE_VERSION_TAG.len() + LONG_TYPE_TAG.len()..].copy_from_slice(&2_u32.to_be_bytes());
	let mut left = store_view(&store);
	let mut right = store_view(&changed);
	let report =
		compare_with_finder_layout(&mut left, &mut right, Limits::default(), false, || false)
			.unwrap();
	let change = &report.changes[0];
	assert_eq!(change.fields, [Field::FinderLayout]);
	assert_eq!(change.finder_layout.len(), 1);
	let record = &change.finder_layout[0];
	assert_eq!(record.name, FOLDER_RECORD_NAME);
	assert_eq!(record.field, STORE_VERSION_TAG);
	assert_eq!(record.kind, ChangeKind::Changed);
	assert!(serde_json::to_value(record).unwrap().get("value").is_none());
}

#[test]
fn compares_opaque_record_identity_and_omits_its_value() {
	let mut store = write_store(None, &[]).unwrap();
	let mut changed = store.clone();
	change_version_to_opaque(&mut changed);
	let report = compare_with_finder_layout(
		&mut store_view(&store),
		&mut store_view(&changed),
		Limits::default(),
		false,
		|| false,
	)
	.unwrap();
	let records = &report.changes[0].finder_layout;
	assert_eq!(
		records
			.iter()
			.map(|record| (record.field, record.kind))
			.collect::<Vec<_>>(),
		[
			(OPAQUE_TAG, ChangeKind::Added),
			(STORE_VERSION_TAG, ChangeKind::Removed)
		]
	);
	change_version_to_opaque(&mut store);
	let opaque = record(&mut changed, OPAQUE_TAG);
	opaque[OPAQUE_TAG.len() + LONG_TYPE_TAG.len()] ^= 1;
	let report = compare_with_finder_layout(
		&mut store_view(&store),
		&mut store_view(&changed),
		Limits::default(),
		false,
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
}

#[test]
fn refuses_malformed_and_oversized_stores_and_enforces_the_payload_budget() {
	let store = write_store(None, &[]).unwrap();
	let oversized = vec![0; ReadLimits::default().store_bytes + 1];
	let mut left = store_view(&store);
	assert!(matches!(
		compare_with_finder_layout(
			&mut left,
			&mut store_view(&[]),
			Limits::default(),
			false,
			|| false
		),
		Err(Error::RightFinderLayout {
			source: FinderLayoutError::Inspect(_),
			..
		})
	));
	assert!(matches!(
		compare_with_finder_layout(
			&mut store_view(&oversized),
			&mut left,
			Limits::default(),
			false,
			|| false
		),
		Err(Error::LeftFinderLayout {
			source: FinderLayoutError::Inspect(ReadError::Limit(ReadBudget::StoreBytes)),
			..
		})
	));
	let budget = Limits {
		max_bytes: store.len() as u64 - 1,
		..Limits::default()
	};
	assert!(matches!(
		compare_with_finder_layout(&mut left, &mut store_view(&store), budget, false, || false),
		Err(Error::ByteLimit { limit }) if limit == budget.max_bytes
	));
}

#[test]
fn refuses_identical_stores_with_a_missing_background_alias() {
	let mut store = write_store(None, &[]).unwrap();
	let length_offset = store
		.windows(ICON_VIEW_BLOB_HEADER.len())
		.position(|bytes| bytes == ICON_VIEW_BLOB_HEADER)
		.unwrap()
		+ ICON_VIEW_BLOB_HEADER.len();
	let body_offset = length_offset + size_of::<u32>();
	let length = u32::from_be_bytes(store[length_offset..body_offset].try_into().unwrap()) as usize;
	let plist = &mut store[body_offset..body_offset + length];
	// The default icon-view plist writes the background type as its only zero integer.
	let background = [ONE_BYTE_PLIST_INTEGER, DEFAULT_BACKGROUND_TYPE];
	let offset = plist
		.windows(background.len())
		.position(|bytes| bytes == background)
		.unwrap();
	plist[offset + 1] = IMAGE_BACKGROUND_TYPE;

	assert!(matches!(
		compare_with_finder_layout(
			&mut store_view(&store),
			&mut store_view(&store),
			Limits::default(),
			false,
			|| false,
		),
		Err(Error::LeftFinderLayout { source: FinderLayoutError::BackgroundAlias { name }, .. })
			if name == FOLDER_RECORD_NAME
	));
}

fn store_view(store: &[u8]) -> VolumeView<TestTree<'_>> {
	let mut node = TestNode::file(FILE_ID);
	node.data = Some(store);
	view([(STORE_NAME, node)])
}

fn version_record(store: &mut [u8]) -> &mut [u8] {
	record(store, STORE_VERSION_TAG)
}

fn change_version_to_opaque(store: &mut [u8]) {
	version_record(store)[..OPAQUE_TAG.len()].copy_from_slice(&OPAQUE_TAG);
}

fn record(store: &mut [u8], field: [u8; 4]) -> &mut [u8] {
	let tag = [field, LONG_TYPE_TAG].concat();
	let start = store
		.windows(tag.len())
		.position(|bytes| bytes == tag)
		.unwrap();
	&mut store[start..start + tag.len() + size_of::<u32>()]
}
