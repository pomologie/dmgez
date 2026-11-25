use std::io;

use dmgez_vfs::volume::{
	FinderInfo, LookupName, NativeNameKey, NativeNameProfile, ViewError, VolumeView,
};
use serde_json::json;

use crate::{
	Side,
	semantic::{
		ChangeKind, Error, Limits, Options, Outcome, PathMatchError, PathMatching,
		compare_with_options,
	},
};

use super::{FILE_MODE, ROOT, TestNode, TestTree, compare, view, view_with_profile};

const FOLDED: NativeNameProfile = NativeNameProfile::HfsUnicodeFolded;
const BINARY: NativeNameProfile = NativeNameProfile::HfsXBinary;
const LEFT_DIRECTORY: &str = "Folder";
const RIGHT_DIRECTORY: &str = "folder";
const SEPARATE_DIRECTORY: &str = "Separate";
const LEFT_NOTE: &str = "Cafe\u{301}.txt";
const RIGHT_NOTE: &str = "cafe\u{301}.txt";
const LEFT_EDITED: &str = "Edited";
const RIGHT_EDITED: &str = "edited";
const EXECUTE_OWNER: u16 = 0o100;
const LEFT_LINK_ID: u64 = 100;
const RIGHT_LINK_ID: u64 = 200;
const SHARED_LINK_COUNT: u32 = 2;
const UNSUPPORTED_KEY: &str = "unqualified test name";

pub(super) fn test_key(profile: NativeNameProfile, name: LookupName<'_>) -> Option<NativeNameKey> {
	let LookupName::Utf8(text) = name else {
		panic!("the comparison passes losslessly decoded stored text to its backend");
	};
	if text == UNSUPPORTED_KEY {
		return None;
	}
	// Only ASCII case pairs are configured for this fake. Production normalization
	// remains the responsibility of the filesystem adapter.
	let bytes = match profile {
		NativeNameProfile::HfsUnicodeFolded => text
			.to_ascii_lowercase()
			.encode_utf16()
			.flat_map(u16::to_be_bytes)
			.collect(),
		NativeNameProfile::HfsXBinary => text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
		NativeNameProfile::ApfsBinary => text.as_bytes().to_vec(),
		_ => return None,
	};
	Some(NativeNameKey::new(bytes))
}

#[test]
fn pairs_nested_paths_by_native_key_and_preserves_both_spellings() {
	const LEFT_DIR: u32 = 2;
	const RIGHT_DIR: u32 = 7;
	const SEPARATE_DIR: u32 = 10;
	let mut edited = TestNode::file(9);
	edited.mode = FILE_MODE | EXECUTE_OWNER;
	let mut left = native_view(
		FOLDED,
		[
			(ROOT, LEFT_DIRECTORY, TestNode::directory(LEFT_DIR)),
			(LEFT_DIR, LEFT_NOTE, TestNode::file(3)),
			(LEFT_DIR, LEFT_EDITED, TestNode::file(4)),
			(ROOT, SEPARATE_DIRECTORY, TestNode::directory(SEPARATE_DIR)),
			(SEPARATE_DIR, LEFT_NOTE, TestNode::file(11)),
		],
	);
	let mut right = native_view(
		FOLDED,
		[
			(ROOT, RIGHT_DIRECTORY, TestNode::directory(RIGHT_DIR)),
			(RIGHT_DIR, RIGHT_NOTE, TestNode::file(8)),
			(RIGHT_DIR, RIGHT_EDITED, edited),
			(ROOT, SEPARATE_DIRECTORY, TestNode::directory(SEPARATE_DIR)),
			(SEPARATE_DIR, LEFT_NOTE, TestNode::file(12)),
		],
	);

	let exact =
		serde_json::to_value(compare(&mut left, &mut right, Limits::default(), || false).unwrap())
			.unwrap();
	assert_eq!(exact["policy"]["paths"], "exact_stored_text");
	assert!(exact["policy"].get("native_name_profile").is_none());
	assert!(exact.get("equivalent_path_spellings").is_none());
	assert_eq!(exact["changes"].as_array().unwrap().len(), 6);

	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		native_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	let report = serde_json::to_value(report).unwrap();
	assert_eq!(report["policy"]["paths"], "native");
	assert_eq!(report["policy"]["native_name_profile"], FOLDED.as_str());
	assert_eq!(
		report["changes"],
		json!([{
			"path": [LEFT_DIRECTORY, LEFT_EDITED],
			"right_path": [RIGHT_DIRECTORY, RIGHT_EDITED],
			"kind": "changed",
			"fields": ["mode"],
		}])
	);
	assert_eq!(
		report["equivalent_path_spellings"],
		json!([
			{"left": [LEFT_DIRECTORY], "right": [RIGHT_DIRECTORY]},
			{"left": [LEFT_DIRECTORY, LEFT_NOTE], "right": [RIGHT_DIRECTORY, RIGHT_NOTE]},
		])
	);
}

#[test]
fn attributes_a_right_fork_read_failure_to_its_stored_path() {
	let mut failing = TestNode::file(3);
	failing.fork_read_failure = true;
	let mut left = native_view(FOLDED, [(ROOT, LEFT_EDITED, TestNode::file(2))]);
	let mut right = native_view(FOLDED, [(ROOT, RIGHT_EDITED, failing)]);
	assert!(matches!(
		compare_with_options(
			&mut left,
			&mut right,
			Limits::default(),
			native_options(),
			|| false,
		),
		Err(Error::RightObject { path, source: ViewError::Backend(error) })
			if path == [RIGHT_EDITED] && error.kind() == io::ErrorKind::UnexpectedEof
	));
}

#[test]
fn preserves_binary_profile_case_distinctions() {
	let mut left = native_view(BINARY, [(ROOT, LEFT_EDITED, TestNode::file(2))]);
	let mut right = native_view(BINARY, [(ROOT, RIGHT_EDITED, TestNode::file(3))]);
	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		native_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(report.changes.len(), 2);
	assert_eq!(report.changes[0].path, [LEFT_EDITED]);
	assert_eq!(report.changes[0].kind, ChangeKind::Removed);
	assert_eq!(report.changes[1].path, [RIGHT_EDITED]);
	assert_eq!(report.changes[1].kind, ChangeKind::Added);
}

#[test]
fn rejects_missing_profiles_and_refuses_to_mix_qualified_profiles() {
	let mut left = native_view(FOLDED, [(ROOT, LEFT_EDITED, TestNode::file(2))]);
	let mut unavailable = view([(RIGHT_EDITED, TestNode::file(3))]);
	assert!(matches!(
		compare_with_options(
			&mut left,
			&mut unavailable,
			Limits::default(),
			native_options(),
			|| false
		),
		Err(Error::PathMatching(PathMatchError::UnavailableProfile {
			side: Side::Right
		}))
	));
	let mut incompatible = native_view(BINARY, [(ROOT, RIGHT_EDITED, TestNode::file(3))]);
	assert!(matches!(
		compare_with_options(
			&mut left,
			&mut incompatible,
			Limits::default(),
			native_options(),
			|| false
		),
		Err(Error::PathMatching(PathMatchError::IncompatibleProfiles {
			left: FOLDED,
			right: BINARY
		}))
	));
}

#[test]
fn diagnoses_ambiguous_stored_aliases_before_comparing_any_payload() {
	const DIRECTORY: u32 = 2;
	let mut left = native_view(
		FOLDED,
		[
			(ROOT, LEFT_DIRECTORY, TestNode::directory(DIRECTORY)),
			(DIRECTORY, RIGHT_EDITED, TestNode::file(3)),
			(DIRECTORY, LEFT_EDITED, TestNode::file(4)),
		],
	);
	let mut right = native_view(FOLDED, []);
	let limits = Limits {
		max_bytes: 0,
		..Limits::default()
	};
	assert!(matches!(
		compare_with_options(&mut left, &mut right, limits, native_options(), || false),
		Err(Error::PathMatching(PathMatchError::Ambiguous { side: Side::Left, first, second }))
			if first == [LEFT_DIRECTORY, LEFT_EDITED] && second == [LEFT_DIRECTORY, RIGHT_EDITED]
	));
}

#[test]
fn rejects_a_missing_native_key_from_an_admitted_profile() {
	let mut left = native_view(FOLDED, []);
	let mut right = native_view(FOLDED, [(ROOT, UNSUPPORTED_KEY, TestNode::file(2))]);
	assert!(matches!(
		compare_with_options(&mut left, &mut right, Limits::default(), native_options(), || false),
		Err(Error::PathMatching(PathMatchError::UnavailableKey { side: Side::Right, path }))
			if path == [UNSUPPORTED_KEY]
	));
}

#[test]
fn independently_meters_stored_paths_and_larger_native_keys() {
	const ROOT_AND_LEADING_SEPARATOR_BYTES: usize = 2;
	let mut left = native_view(FOLDED, []);
	let mut right = native_view(FOLDED, [(ROOT, LEFT_EDITED, TestNode::file(2))]);
	let limits = Limits {
		max_path_bytes: ROOT_AND_LEADING_SEPARATOR_BYTES + LEFT_EDITED.len(),
		..Limits::default()
	};
	assert_eq!(
		compare(&mut left, &mut right, limits, || false)
			.unwrap()
			.outcome,
		Outcome::Different
	);
	assert!(matches!(
		compare_with_options(&mut left, &mut right, limits, native_options(), || false),
		Err(Error::PathMatching(PathMatchError::KeyBytesLimit { side: Side::Right, limit }))
			if limit == limits.max_path_bytes
	));
}

#[test]
fn compares_hard_link_groups_across_reordered_stored_spellings() {
	let mut left = native_view(
		FOLDED,
		[
			(
				ROOT,
				"Beta",
				TestNode::file(2).linked(LEFT_LINK_ID, SHARED_LINK_COUNT),
			),
			(
				ROOT,
				"alpha",
				TestNode::file(3).linked(LEFT_LINK_ID, SHARED_LINK_COUNT),
			),
		],
	);
	let mut right = native_view(
		FOLDED,
		[
			(
				ROOT,
				"ALPHA",
				TestNode::file(7).linked(RIGHT_LINK_ID, SHARED_LINK_COUNT),
			),
			(
				ROOT,
				"beta",
				TestNode::file(8).linked(RIGHT_LINK_ID, SHARED_LINK_COUNT),
			),
		],
	);
	let report = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		native_options(),
		|| false,
	)
	.unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert!(report.changes.is_empty());
	let report = serde_json::to_value(report).unwrap();
	assert_eq!(
		report["equivalent_path_spellings"],
		json!([
			{"left": ["Beta"], "right": ["beta"]},
			{"left": ["alpha"], "right": ["ALPHA"]},
		])
	);
}

#[cfg(feature = "content-hash")]
#[test]
fn infers_renames_only_among_paths_left_unmatched_by_native_keys() {
	const OLD: &str = "old.bin";
	const NEW: &str = "new.bin";
	let mut left = native_view(
		FOLDED,
		[
			(ROOT, LEFT_EDITED, TestNode::file(2)),
			(ROOT, OLD, TestNode::file(3)),
		],
	);
	let mut right = native_view(
		FOLDED,
		[
			(ROOT, RIGHT_EDITED, TestNode::file(7)),
			(ROOT, NEW, TestNode::file(8)),
		],
	);
	let options = Options {
		infer_renames: true,
		..native_options()
	};
	let report =
		compare_with_options(&mut left, &mut right, Limits::default(), options, || false).unwrap();
	assert_eq!(report.changes.len(), 2);
	assert_eq!(report.rename_candidates.len(), 1);
	assert_eq!(report.rename_candidates[0].from, [OLD]);
	assert_eq!(report.rename_candidates[0].to, [NEW]);
}

fn native_view<'data, const N: usize>(
	profile: NativeNameProfile,
	children: [(u32, &str, TestNode<'data>); N],
) -> VolumeView<TestTree<'data>> {
	view_with_profile(children, true, FinderInfo::Absent, Some(profile))
}

fn native_options() -> Options {
	Options {
		paths: PathMatching::Native,
		..Options::default()
	}
}
