use dmgez_vfs::volume::{FINDER_INFO_BYTES, FinderInfo};
use serde_json::json;

use super::{Error, Field, Limits, Outcome, TestNode, compare, view, view_with_metadata};
use crate::semantic::{FinderInfoError, Options, Uncompared, compare_with_options};

const ITEM: &str = "item";
const ITEM_ID: u32 = 2;
const EMPTY_RECORD: FinderInfo = FinderInfo::Present([0; FINDER_INFO_BYTES]);
const MARKED_RECORD: FinderInfo = FinderInfo::Present([0x5a; FINDER_INFO_BYTES]);
const TARGET: &[u8] = b"elsewhere";

const FINDER_INFO: Options = Options {
	xattrs: false,
	finder_info: true,
	#[cfg(feature = "finder-layout")]
	finder_layout: false,
};

#[test]
fn reports_root_and_link_finder_info_without_returning_record_bytes() {
	let mut marked_link = TestNode::link(ITEM_ID, TARGET);
	marked_link.finder_info = MARKED_RECORD;
	let mut left = view([(ITEM, TestNode::link(ITEM_ID, TARGET))]);
	let mut right = view_with_metadata([(ITEM, marked_link)], true, EMPTY_RECORD);

	let baseline = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(baseline.outcome, Outcome::Equal);
	let json = serde_json::to_value(baseline).unwrap();
	assert!(json["policy"].get("finder_info").is_none());
	assert!(
		json["policy"]["excluded"]
			.as_array()
			.unwrap()
			.contains(&json!("finder_metadata"))
	);

	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		FINDER_INFO,
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(
		report.compared_bytes,
		(TARGET.len() + 2 * FINDER_INFO_BYTES) as u64
	);
	assert!(report.policy.compared.contains(&Field::FinderInfo));
	assert!(report.policy.excluded.contains(&Uncompared::FinderLayout));
	assert!(!report.policy.excluded.contains(&Uncompared::FinderMetadata));
	let json = serde_json::to_value(report).unwrap();
	assert_eq!(
		json["policy"]["finder_info"],
		json!({
			"comparison": "exact_effective_bytes",
			"record_bytes": FINDER_INFO_BYTES,
			"same_kind_only": true,
		})
	);
	assert_eq!(
		json["changes"],
		json!([
			{"path": [], "kind": "changed", "fields": ["finder_info"]},
			{"path": [ITEM], "kind": "changed", "fields": ["finder_info"]},
		])
	);
}

#[test]
fn charges_equal_present_pairs_once_and_absent_pairs_zero_bytes() {
	let mut left = view_with_metadata([], true, MARKED_RECORD);
	let mut right = view_with_metadata([], true, MARKED_RECORD);
	let exact = Limits {
		max_bytes: FINDER_INFO_BYTES as u64,
		..Limits::default()
	};
	let report = compare_with_options(&mut left, &mut right, exact, FINDER_INFO, || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert_eq!(report.compared_bytes, exact.max_bytes);
	let short = Limits {
		max_bytes: exact.max_bytes - 1,
		..exact
	};
	assert!(matches!(
		compare_with_options(&mut left, &mut right, short, FINDER_INFO, || false),
		Err(Error::ByteLimit { limit }) if limit == short.max_bytes
	));
	let zero = Limits {
		max_bytes: 0,
		..exact
	};
	let absent =
		compare_with_options(&mut view([]), &mut view([]), zero, FINDER_INFO, || false).unwrap();
	assert_eq!(absent.compared_bytes, 0);
}

#[test]
fn refuses_unavailable_records_but_does_not_query_differing_kinds() {
	let mut unsupported = TestNode::file(ITEM_ID);
	unsupported.finder_info = FinderInfo::Unavailable;
	let mut right = view([(ITEM, unsupported)]);
	assert!(matches!(
		compare_with_options(
			&mut view([(ITEM, TestNode::file(ITEM_ID))]), &mut right, Limits::default(), FINDER_INFO, || false,
		),
		Err(Error::RightFinderInfo { path, source: FinderInfoError::Unavailable }) if path == [ITEM],
	));
	let report = compare_with_options(
		&mut view([(ITEM, TestNode::link(ITEM_ID, TARGET))]),
		&mut right,
		Limits::default(),
		FINDER_INFO,
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert!(report.changes[0].fields.contains(&Field::Kind));
	assert!(!report.changes[0].fields.contains(&Field::FinderInfo));
}
