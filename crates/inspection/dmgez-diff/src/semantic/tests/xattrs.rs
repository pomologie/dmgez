use dmgez_vfs::volume::RawName;

use super::{
	ChangeKind, Error, Field, Limits, Outcome, TestNode, TestXattr, view, view_with_attributes,
};
use crate::semantic::{XattrError, compare, compare_with_xattrs};

const FILE: &str = "item";
const VALUE: &str = "com.example.Value";
const EMPTY: &str = "com.example.Empty";
const LOWERCASE_EMPTY: &str = "com.example.empty";
const MARKER: &str = "com.apple.quarantine";
const RESOURCE_FORK: &str = "com.apple.ResourceFork";
const FINDER_INFO: &str = "com.apple.FinderInfo";
const COMPRESSION_RECORD: &str = "com.apple.decmpfs";
const SYMLINK_RECORD: &str = "com.apple.fs.symlink";
const ACCESS_CONTROL_LIST: &str = "com.apple.system.Security";
const OLD: &[u8] = b"left-private-value";
const NEW: &[u8] = b"rght-private-value";

#[test]
fn reports_exact_attribute_names_and_kinds_without_payloads() {
	let mut left = view([(
		FILE,
		with_attrs([
			attr(VALUE, Some(OLD)),
			attr(EMPTY, Some(b"")),
			attr(MARKER, Some(OLD)),
		]),
	)]);
	let mut right = view([(
		FILE,
		with_attrs([attr(VALUE, Some(NEW)), attr(LOWERCASE_EMPTY, Some(b""))]),
	)]);
	let unchanged = compare(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(unchanged.outcome, Outcome::Equal);
	assert!(
		!serde_json::to_value(unchanged).unwrap()["policy"]
			.as_object()
			.unwrap()
			.contains_key("xattrs")
	);

	let report = compare_with_xattrs(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(report.changes[0].fields, [Field::ExtendedAttributes]);
	let changes: Vec<_> = report.changes[0]
		.xattrs
		.iter()
		.map(|change| (change.name.as_str(), change.kind))
		.collect();
	assert_eq!(
		changes,
		[
			(MARKER, ChangeKind::Removed),
			(EMPTY, ChangeKind::Removed),
			(VALUE, ChangeKind::Changed),
			(LOWERCASE_EMPTY, ChangeKind::Added),
		]
	);
	assert_eq!(
		report.compared_bytes,
		(super::SHARED_BYTE_CONTENT.len() + OLD.len()) as u64
	);
	let json = serde_json::to_string(&report).unwrap();
	for secret in [OLD, NEW] {
		assert!(!json.contains(std::str::from_utf8(secret).unwrap()));
	}
}

#[test]
fn internal_attribute_names_are_reported_as_excluded_and_never_opened() {
	let internal = [
		RESOURCE_FORK,
		FINDER_INFO,
		COMPRESSION_RECORD,
		SYMLINK_RECORD,
		ACCESS_CONTROL_LIST,
	];
	let mut left = view([(FILE, with_attrs(internal.map(|name| attr(name, None))))]);
	let mut right = view([(FILE, TestNode::file(3))]);
	let report = compare_with_xattrs(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert_eq!(report.policy.xattrs.unwrap().excluded_names, internal);
}

#[test]
fn uses_native_name_encoding_to_open_each_side() {
	const UNICODE: &str = "com.example.関";
	let mut left = view([(FILE, with_attrs([attr(UNICODE, Some(b""))]))]);
	let mut right = view([(
		FILE,
		with_attrs([TestXattr {
			name: RawName::Utf16(UNICODE.encode_utf16().collect()),
			value: Some(b""),
		}]),
	)]);
	let report = compare_with_xattrs(&mut left, &mut right, Limits::default(), || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
}

#[test]
fn attribute_values_share_the_existing_payload_budget() {
	let mut left = view([(FILE, with_attrs([attr(VALUE, Some(OLD))]))]);
	let mut right = view([(FILE, with_attrs([attr(VALUE, Some(OLD))]))]);
	let limits = Limits {
		max_bytes: (super::SHARED_BYTE_CONTENT.len() + OLD.len() - 1) as u64,
		..Limits::default()
	};
	assert!(matches!(
		compare_with_xattrs(&mut left, &mut right, limits, || false),
		Err(Error::ByteLimit { limit }) if limit == limits.max_bytes
	));
}

#[test]
fn missing_or_unavailable_attributes_identify_the_side_that_failed() {
	let mut left = view([(FILE, with_attrs([attr(VALUE, Some(OLD))]))]);
	let mut right = view([(FILE, with_attrs([attr(VALUE, None)]))]);
	assert!(matches!(
		compare_with_xattrs(&mut left, &mut right, Limits::default(), || false),
		Err(Error::RightXattrs {
			path,
			source: XattrError::Missing { name },
		}) if path == [FILE] && name == VALUE
	));
	let mut left = view_with_attributes([], false);
	let mut right = view([]);
	assert!(matches!(
		compare_with_xattrs(&mut left, &mut right, Limits::default(), || false),
		Err(Error::LeftXattrs {
			path,
			source: XattrError::Unavailable,
		}) if path.is_empty()
	));
}

fn attr(name: &str, value: Option<&'static [u8]>) -> TestXattr {
	TestXattr {
		name: RawName::Utf8(name.to_owned()),
		value,
	}
}

fn with_attrs<const N: usize>(xattrs: [TestXattr; N]) -> TestNode<'static> {
	TestNode {
		xattrs: xattrs.into(),
		..TestNode::file(2)
	}
}
