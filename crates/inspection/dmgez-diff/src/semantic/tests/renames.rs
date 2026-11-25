use serde_json::json;

use super::{ChangeKind, Error, Limits, Outcome, SHARED_BYTE_CONTENT, TestNode, compare, view};
use crate::semantic::{Options, Uncompared, compare_with_options};

const UNIQUE_FIRST: &[u8] = b"first unique file";
const UNIQUE_SECOND: &[u8] = b"second unique file";
const AMBIGUOUS_LEFT: &[u8] = b"two removed copies";
const AMBIGUOUS_RIGHT: &[u8] = b"two added copies";
const RESOURCE_ONLY: &[u8] = b"resource without file data";
const LEFT_ONLY: &[u8] = b"long unmatched stream";
const RIGHT_ONLY: &[u8] = b"short stream";
const COMMON: &str = "common";
const OLD: &str = "old";
const NEW: &str = "new";
const SHARED_STORAGE: u64 = 100;

#[test]
fn keeps_unmatched_changes_and_reports_unique_candidates_in_stored_path_order() {
	let mut left = view([
		("zz-old", file(2, UNIQUE_SECOND)),
		("aa-old", file(3, UNIQUE_FIRST)),
		("copy-one", file(4, AMBIGUOUS_LEFT)),
		("copy-two", file(5, AMBIGUOUS_LEFT)),
		("source-copy", file(6, AMBIGUOUS_RIGHT)),
	]);
	let mut right = view([
		("copy-destination", file(12, AMBIGUOUS_LEFT)),
		("aa-new", file(13, UNIQUE_SECOND)),
		("zz-new", file(14, UNIQUE_FIRST)),
		("destination-one", file(15, AMBIGUOUS_RIGHT)),
		("destination-two", file(16, AMBIGUOUS_RIGHT)),
	]);
	let baseline = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert!(
		baseline
			.policy
			.excluded
			.contains(&Uncompared::RenameInference)
	);
	let baseline = serde_json::to_value(baseline).unwrap();
	assert!(baseline.get("rename_candidates").is_none());
	assert!(baseline["policy"].get("rename_inference").is_none());

	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		rename_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert!(
		!report
			.policy
			.excluded
			.contains(&Uncompared::RenameInference)
	);
	let report = serde_json::to_value(report).unwrap();
	assert_eq!(report["changes"], baseline["changes"]);
	assert_eq!(report["policy"]["rename_inference"], "unique_blake3_forks");
	assert_eq!(
		report["rename_candidates"],
		json!([
			{"from": ["aa-old"], "to": ["zz-new"]},
			{"from": ["zz-old"], "to": ["aa-new"]},
		]),
	);
}

#[test]
fn uses_nonempty_resource_only_content_and_distinguishes_absent_from_present_empty() {
	let mut left = view([
		(
			"resource-old",
			file_with_resource(2, b"", Some(RESOURCE_ONLY)),
		),
		("resource-absent", file(3, SHARED_BYTE_CONTENT)),
		("empty-old", file(4, b"")),
	]);
	let mut right = view([
		(
			"resource-new",
			file_with_resource(12, b"", Some(RESOURCE_ONLY)),
		),
		(
			"resource-empty",
			file_with_resource(13, SHARED_BYTE_CONTENT, Some(b"")),
		),
		("empty-new", file(14, b"")),
	]);
	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		rename_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.changes.len(), 6);
	assert_eq!(report.rename_candidates.len(), 1);
	assert_eq!(report.rename_candidates[0].from, ["resource-old"]);
	assert_eq!(report.rename_candidates[0].to, ["resource-new"]);
}

#[test]
fn omits_shared_aliases_and_supports_an_admitted_singleton_storage_record() {
	let mut left = view([(OLD, TestNode::file(2).linked(SHARED_STORAGE, 2))]);
	let mut right = view([(NEW, TestNode::file(3))]);
	let no_reads = Limits {
		max_bytes: 0,
		..Limits::default()
	};
	let report =
		compare_with_options(&mut left, &mut right, no_reads, rename_options(), || false).unwrap();
	assert!(report.rename_candidates.is_empty());
	assert_eq!(report.compared_bytes, 0);

	let mut left = view([(OLD, TestNode::file(2).linked(SHARED_STORAGE, 1))]);
	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		rename_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.rename_candidates.len(), 1);
	assert_eq!(report.rename_candidates[0].from, [OLD]);
	assert_eq!(report.rename_candidates[0].to, [NEW]);
	assert_eq!(report.changes[0].kind, ChangeKind::Added);
	assert_eq!(report.changes[1].kind, ChangeKind::Removed);
}

#[test]
fn rename_scans_share_the_budget_and_charge_the_larger_unpaired_side_total() {
	let mut left = view([
		(COMMON, TestNode::file(2)),
		(OLD, file(3, UNIQUE_FIRST)),
		("unmatched-old", file(4, LEFT_ONLY)),
	]);
	let mut right = view([
		(COMMON, TestNode::file(12)),
		(NEW, file(13, UNIQUE_FIRST)),
		("unmatched-new", file(14, RIGHT_ONLY)),
	]);
	let full = Limits {
		max_bytes: (SHARED_BYTE_CONTENT.len() + UNIQUE_FIRST.len() + LEFT_ONLY.len()) as u64,
		..Limits::default()
	};
	let short = Limits {
		max_bytes: full.max_bytes - 1,
		..full
	};
	assert!(matches!(
		compare_with_options(&mut left, &mut right, short, rename_options(), || false),
		Err(Error::ByteLimit { limit }) if limit == short.max_bytes
	));
	let report =
		compare_with_options(&mut left, &mut right, full, rename_options(), || false).unwrap();
	assert_eq!(report.compared_bytes, full.max_bytes);
	assert_eq!(report.rename_candidates.len(), 1);
}

fn file(id: u32, data: &'static [u8]) -> TestNode<'static> {
	file_with_resource(id, data, None)
}

fn file_with_resource(
	id: u32,
	data: &'static [u8],
	resource: Option<&'static [u8]>,
) -> TestNode<'static> {
	TestNode {
		data: Some(data),
		resource,
		..TestNode::file(id)
	}
}

fn rename_options() -> Options {
	Options {
		infer_renames: true,
		..Options::default()
	}
}
