//! Exact container and logical extent coverage for D-REL-03-T01.

use dmgez_diff::{DEFAULT_COMPARE_BYTES, Error, Outcome, Side, compare};
use dmgez_udif::{
	Error as SourceError,
	reader::{Image, Limits, ReadAt},
};
use dmgez_vfs::{LogicalDisk, Raw};

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const NATIVE_LOGICAL_BYTES: u64 = 2048;
const WINDOW_BYTES: usize = 64 * 1024;
const DATA_BYTES: usize = WINDOW_BYTES + 32;
const COMPARE_LIMIT: u64 = WINDOW_BYTES as u64 + 5;

#[test]
fn native_images_have_equal_decoded_disks_and_different_stored_bytes() {
	let physical = compare(
		&mut Raw(UDRO),
		&mut Raw(UDZO),
		DEFAULT_COMPARE_BYTES,
		|| false,
	)
	.unwrap();
	assert_eq!(physical.outcome, Outcome::ByteMismatch);
	assert_ne!(physical.left_bytes, physical.right_bytes);
	let mut raw = Image::open(UDRO, Limits::default()).unwrap();
	let mut compressed = Image::open(UDZO, Limits::default()).unwrap();
	let logical = compare(&mut raw, &mut compressed, DEFAULT_COMPARE_BYTES, || false).unwrap();
	assert_eq!(logical.outcome, Outcome::Equal);
	assert_eq!(logical.left_bytes, NATIVE_LOGICAL_BYTES);
	assert_eq!(logical.right_bytes, NATIVE_LOGICAL_BYTES);
	assert_eq!(logical.common_prefix_bytes, NATIVE_LOGICAL_BYTES);
	let serialized = serde_json::to_value(logical).unwrap();
	assert_eq!(serialized["outcome"], "equal");
	assert_eq!(
		serialized["common_prefix_bytes"],
		NATIVE_LOGICAL_BYTES.to_string()
	);
}

#[test]
fn finds_the_first_difference_or_the_matching_prefix_at_eof() {
	let left = vec![7; DATA_BYTES];
	let mut right = left.clone();
	let first_mismatch = WINDOW_BYTES + 3;
	right[first_mismatch] = 8;
	right[first_mismatch + 1] = 9;
	let mismatch = compare(
		&mut Raw(left.as_slice()),
		&mut Raw(right.as_slice()),
		COMPARE_LIMIT,
		|| false,
	)
	.unwrap();
	assert_eq!(mismatch.outcome, Outcome::ByteMismatch);
	assert_eq!(mismatch.common_prefix_bytes, first_mismatch as u64);
	let prefix = compare(
		&mut Raw(left.as_slice()),
		&mut Raw(&right[..WINDOW_BYTES]),
		WINDOW_BYTES as u64,
		|| false,
	)
	.unwrap();
	assert_eq!(prefix.outcome, Outcome::LengthMismatch);
	assert_eq!(prefix.common_prefix_bytes, WINDOW_BYTES as u64);
	let empty = compare(&mut Raw(&[][..]), &mut Raw(&[][..]), 0, || false).unwrap();
	assert_eq!(empty.outcome, Outcome::Equal);
}

struct Observed<'data> {
	bytes: &'data [u8],
	reads: Vec<(u64, usize)>,
	fail_identity_check: bool,
}

impl LogicalDisk for Observed<'_> {
	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		self.reads.push((offset, output.len()));
		if output.is_empty() && self.fail_identity_check {
			return Err(SourceError::SourceChanged);
		}
		self.bytes.read_exact_at(offset, output)
	}
}

#[test]
fn checks_source_identity_and_never_reads_payload_past_the_budget() {
	let data = vec![7; DATA_BYTES];
	let mut left = Observed {
		bytes: &data,
		reads: Vec::new(),
		fail_identity_check: false,
	};
	let mut right = Observed {
		bytes: &data,
		reads: Vec::new(),
		fail_identity_check: true,
	};
	assert!(matches!(
		compare(&mut left, &mut right, COMPARE_LIMIT, || false),
		Err(Error::Limit {
			limit: COMPARE_LIMIT
		})
	));
	let bounded_reads = [(0, WINDOW_BYTES), (WINDOW_BYTES as u64, 5)];
	assert_eq!(left.reads, bounded_reads);
	assert_eq!(right.reads, bounded_reads);
	assert!(matches!(
		compare(&mut left, &mut right, DEFAULT_COMPARE_BYTES, || false),
		Err(Error::Read { side: Side::Right, offset, source: dmgez_vfs::Error::Source(SourceError::SourceChanged) }) if offset == DATA_BYTES as u64
	));
	assert_eq!(left.reads.last(), Some(&(DATA_BYTES as u64, 0)));
	assert_eq!(right.reads.last(), Some(&(DATA_BYTES as u64, 0)));
	let before = left.reads.len();
	assert!(matches!(
		compare(&mut left, &mut right, DEFAULT_COMPARE_BYTES, || true),
		Err(Error::Cancelled)
	));
	assert_eq!(left.reads.len(), before);
}
