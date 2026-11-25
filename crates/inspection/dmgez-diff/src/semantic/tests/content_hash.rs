use std::cell::Cell;

use dmgez_vfs::volume::RawName;

use super::{Error, Field, Limits, Outcome, SHARED_BYTE_CONTENT, TestNode, TestXattr, view};
use crate::semantic::{ContentComparison, Contents, Options, compare_with_options};

const FILE: &str = "item";
const LEFT_ID: u32 = 2;
const RIGHT_ID: u32 = 3;
const LONG_CONTENT_BYTES: usize = 128 * 1024 + 1;
const ORIGINAL_BYTE: u8 = b'a';
const REVISED_BYTE: u8 = b'z';
const ORIGINAL_RESOURCE: &[u8] = b"original resource fork";
const REVISED_RESOURCE: &[u8] = b"modified resource fork";
const XATTR_NAME: &str = "com.example.comment";
const XATTR_VALUE: &[u8] = b"one exact pass";
const LINK_STORAGE: u64 = 45;
const LINK_COUNT: u32 = 2;
const LEFT_LINK_DATA: &[u8] = b"left-stream";
const RIGHT_LINK_DATA: &[u8] = b"rght-stream";
const LINK_RESOURCE: &[u8] = b"common-fork";

#[test]
fn hashes_late_data_and_resource_changes_without_a_confirmation_pass() {
	let original = vec![ORIGINAL_BYTE; LONG_CONTENT_BYTES];
	let mut revised = original.clone();
	*revised.last_mut().unwrap() = REVISED_BYTE;
	let left_reads = Cell::new(0);
	let right_reads = Cell::new(0);
	let mut left = view([(
		FILE,
		TestNode {
			resource: Some(ORIGINAL_RESOURCE),
			..tracked_file(LEFT_ID, &original, &left_reads)
		},
	)]);
	let mut right = view([(
		FILE,
		TestNode {
			resource: Some(REVISED_RESOURCE),
			..tracked_file(RIGHT_ID, &revised, &right_reads)
		},
	)]);
	let limits = Limits {
		max_bytes: (LONG_CONTENT_BYTES + ORIGINAL_RESOURCE.len()) as u64,
		..Limits::default()
	};
	for (contents, policy) in [
		(Contents::Blake3, ContentComparison::Blake3),
		(Contents::Blake3Exact, ContentComparison::Blake3Exact),
	] {
		left_reads.set(0);
		right_reads.set(0);
		let report = compare_with_options(
			&mut left,
			&mut right,
			limits,
			hash_options(contents),
			|| false,
		)
		.unwrap();
		assert_eq!(report.outcome, Outcome::Different);
		assert_eq!(report.policy.contents, policy);
		assert_eq!(report.compared_bytes, limits.max_bytes);
		assert_eq!(left_reads.get(), limits.max_bytes);
		assert_eq!(right_reads.get(), limits.max_bytes);
		assert_eq!(report.changes.len(), 1);
		assert_eq!(
			report.changes[0].fields,
			[Field::DataFork, Field::ResourceFork]
		);
	}
}

#[test]
fn equal_digests_use_one_pass_and_exact_confirmation_reads_and_charges_a_second_pass() {
	let bytes = [ORIGINAL_BYTE; LONG_CONTENT_BYTES];
	let left_reads = Cell::new(0);
	let right_reads = Cell::new(0);
	let mut left = view([(FILE, tracked_file(LEFT_ID, &bytes, &left_reads))]);
	let mut right = view([(FILE, tracked_file(RIGHT_ID, &bytes, &right_reads))]);
	let hashed = Limits {
		max_bytes: LONG_CONTENT_BYTES as u64,
		..Limits::default()
	};
	let hash_options_only = hash_options(Contents::Blake3);
	let short_hash = Limits {
		max_bytes: hashed.max_bytes - 1,
		..hashed
	};
	assert!(matches!(
		compare_with_options(&mut left, &mut right, short_hash, hash_options_only, || false),
		Err(Error::ByteLimit { limit }) if limit == short_hash.max_bytes
	));
	assert_eq!(left_reads.get(), 0);
	assert_eq!(right_reads.get(), 0);
	let hash =
		compare_with_options(&mut left, &mut right, hashed, hash_options_only, || false).unwrap();
	assert_eq!(hash.outcome, Outcome::Equal);
	assert_eq!(hash.compared_bytes, hashed.max_bytes);
	assert_eq!(left_reads.get(), hashed.max_bytes);
	assert_eq!(right_reads.get(), hashed.max_bytes);

	let exact = Limits {
		max_bytes: 2 * hashed.max_bytes,
		..hashed
	};
	let short = Limits {
		max_bytes: exact.max_bytes - 1,
		..exact
	};
	let confirmed = hash_options(Contents::Blake3Exact);
	assert!(matches!(
		compare_with_options(&mut left, &mut right, short, confirmed, || false),
		Err(Error::ByteLimit { limit }) if limit == short.max_bytes
	));
	left_reads.set(0);
	right_reads.set(0);
	let report = compare_with_options(&mut left, &mut right, exact, confirmed, || false).unwrap();
	assert_eq!(report.outcome, Outcome::Equal);
	assert_eq!(report.compared_bytes, exact.max_bytes);
	assert_eq!(left_reads.get(), exact.max_bytes);
	assert_eq!(right_reads.get(), exact.max_bytes);
}

#[test]
fn confirmation_leaves_extended_attributes_exact_and_preserves_empty_resource_presence() {
	let mut left = view([(FILE, with_attribute(LEFT_ID, None))]);
	let mut right = view([(FILE, with_attribute(RIGHT_ID, Some(b"")))]);
	let options = Options {
		contents: Contents::Blake3Exact,
		xattrs: true,
		..Options::default()
	};
	let limits = Limits {
		max_bytes: (2 * SHARED_BYTE_CONTENT.len() + XATTR_VALUE.len()) as u64,
		..Limits::default()
	};
	let report = compare_with_options(&mut left, &mut right, limits, options, || false).unwrap();
	assert_eq!(report.outcome, Outcome::Different);
	assert_eq!(report.changes[0].fields, [Field::ResourceFork]);
	assert_eq!(report.compared_bytes, limits.max_bytes);
}

#[test]
fn cancellation_stops_a_hash_before_reading_all_chunks() {
	let content = vec![ORIGINAL_BYTE; LONG_CONTENT_BYTES];
	let left_reads = Cell::new(0);
	let right_reads = Cell::new(0);
	let mut left = view([(FILE, tracked_file(LEFT_ID, &content, &left_reads))]);
	let mut right = view([(FILE, tracked_file(RIGHT_ID, &content, &right_reads))]);
	let result = compare_with_options(
		&mut left,
		&mut right,
		Limits::default(),
		hash_options(Contents::Blake3),
		|| left_reads.get() > 0,
	);
	assert!(matches!(result, Err(Error::Cancelled)));
	assert!((1..LONG_CONTENT_BYTES as u64).contains(&left_reads.get()));
}

#[test]
fn caches_link_fingerprints_per_view_and_fork_kind() {
	let left_reads = Cell::new(0);
	let right_reads = Cell::new(0);
	let mut left = view([
		("first alias", tracked_alias(2, LEFT_LINK_DATA, &left_reads)),
		(
			"second alias",
			tracked_alias(3, LEFT_LINK_DATA, &left_reads),
		),
	]);
	let mut right = view([
		(
			"first alias",
			tracked_alias(4, RIGHT_LINK_DATA, &right_reads),
		),
		(
			"second alias",
			tracked_alias(5, RIGHT_LINK_DATA, &right_reads),
		),
	]);
	let fork_bytes = (LEFT_LINK_DATA.len() + LINK_RESOURCE.len()) as u64;
	let exact_resource_bytes = u64::from(LINK_COUNT) * LINK_RESOURCE.len() as u64;
	for (contents, expected) in [
		(Contents::Blake3, fork_bytes),
		(Contents::Blake3Exact, fork_bytes + exact_resource_bytes),
		(Contents::Exact, u64::from(LINK_COUNT) * fork_bytes),
	] {
		left_reads.set(0);
		right_reads.set(0);
		let limits = Limits {
			max_bytes: expected,
			..Limits::default()
		};
		let report = compare_with_options(
			&mut left,
			&mut right,
			limits,
			hash_options(contents),
			|| false,
		)
		.unwrap();
		assert_eq!(report.compared_bytes, expected);
		assert_eq!(left_reads.get(), expected);
		assert_eq!(right_reads.get(), expected);
		assert_eq!(report.changes.len(), LINK_COUNT as usize);
		for change in report.changes {
			assert_eq!(change.fields, [Field::DataFork]);
		}
	}
}

fn tracked_file<'data>(
	id: u32,
	bytes: &'data [u8],
	bytes_read: &'data Cell<u64>,
) -> TestNode<'data> {
	TestNode {
		data: Some(bytes),
		bytes_read: Some(bytes_read),
		..TestNode::file(id)
	}
}

fn tracked_alias<'data>(
	id: u32,
	data: &'data [u8],
	bytes_read: &'data Cell<u64>,
) -> TestNode<'data> {
	TestNode {
		resource: Some(LINK_RESOURCE),
		..tracked_file(id, data, bytes_read).linked(LINK_STORAGE, LINK_COUNT)
	}
}

fn with_attribute(id: u32, resource: Option<&'static [u8]>) -> TestNode<'static> {
	TestNode {
		resource,
		xattrs: vec![TestXattr {
			name: RawName::Utf8(XATTR_NAME.into()),
			value: Some(XATTR_VALUE),
		}],
		..TestNode::file(id)
	}
}

fn hash_options(contents: Contents) -> Options {
	Options {
		contents,
		..Options::default()
	}
}
