//! Container space measurements from Apple-generated metadata (D-OPS-02-T01).

use dmgez_inspector::{CheckStatus, ReportOperation, analyze};
use dmgez_udif::{
	Error,
	reader::{Image, Limits, ReadAt},
};

const HYBRID: &[u8] = include_bytes!("../../../../tests/fixtures/images/hybrid-UDZO.dmg");
const RAW: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const HYBRID_STORED_BYTES: u64 = 918;
const HYBRID_ZLIB_LOGICAL_BYTES: u64 = 51_712;
const HYBRID_IGNORE_LOGICAL_BYTES: u64 = 869_888;
const HYBRID_ZLIB_RUNS: usize = 4;
const HYBRID_IGNORE_RUNS: usize = 3;
const RAW_STORED_BYTES: u64 = 2048;

#[test]
fn accounts_native_compressed_and_ignored_sectors_without_payload_io() {
	struct MetadataOnly;
	impl ReadAt for MetadataOnly {
		fn len(&self) -> u64 {
			HYBRID.len() as u64
		}

		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			assert!(offset >= HYBRID_STORED_BYTES, "analysis read data fork");
			HYBRID.read_exact_at(offset, output)
		}
	}
	let image = Image::open(MetadataOnly, Limits::default()).unwrap();
	let report = analyze(&image);
	assert_eq!(report.operation, ReportOperation::Analyze);
	assert_eq!(report.runs.count, HYBRID_ZLIB_RUNS + HYBRID_IGNORE_RUNS);
	assert_eq!(report.runs.stored_bytes, HYBRID_STORED_BYTES);
	assert_eq!(
		report.runs.logical_bytes,
		HYBRID_ZLIB_LOGICAL_BYTES + HYBRID_IGNORE_LOGICAL_BYTES
	);
	assert_eq!(report.by_encoding.zlib.count, HYBRID_ZLIB_RUNS);
	assert_eq!(report.by_encoding.zlib.stored_bytes, HYBRID_STORED_BYTES);
	assert_eq!(
		report.by_encoding.zlib.logical_bytes,
		HYBRID_ZLIB_LOGICAL_BYTES
	);
	assert_eq!(report.by_encoding.ignore.count, HYBRID_IGNORE_RUNS);
	assert_eq!(report.by_encoding.ignore.stored_bytes, 0);
	assert_eq!(
		report.by_encoding.ignore.logical_bytes,
		HYBRID_IGNORE_LOGICAL_BYTES
	);
	assert_eq!(
		report.outside_data_fork_bytes,
		HYBRID.len() as u64 - HYBRID_STORED_BYTES
	);
	assert_eq!(report.unmapped_data_fork_bytes, 0);
	assert_eq!(report.coverage.payload, CheckStatus::NotChecked);
	assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
}

#[test]
fn raw_runs_use_their_entire_logical_length_in_the_data_fork() {
	let image = Image::open(RAW, Limits::default()).unwrap();
	let report = analyze(&image);
	assert_eq!(report.runs, report.by_encoding.raw);
	assert_eq!(report.runs.logical_bytes, RAW_STORED_BYTES);
	assert_eq!(report.runs.stored_bytes, RAW_STORED_BYTES);
	assert_eq!(report.unmapped_data_fork_bytes, 0);
}
