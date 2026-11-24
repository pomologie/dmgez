//! Container portions of D-READ-03-T01 and D-READ-04-T01/T02.

use std::io;

use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, DiagnosticDetail, ErrorCategory, MAX_RUN_PAGE_SIZE,
	RunEncoding, RunWindow, SCHEMA_VERSION, Stage, VerificationMode, VerificationOutcome, inspect,
	verify,
};
use dmgez_udif::{
	Error,
	reader::{Image, Limits, ReadAt},
};
use plist::Value;

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const HYBRID_UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/hybrid-UDZO.dmg");
const ULFO: &[u8] = include_bytes!("../../../formats/dmgez-udif/tests/fixtures/apple-ulfo.dmg");
const ULMO: &[u8] = include_bytes!("../../../formats/dmgez-udif/tests/fixtures/apple-ulmo.dmg");
const NATIVE_LOGICAL_BYTES: u64 = 2048;
const NATIVE_UDZO_FORK_BYTES: u64 = 300;
const TRAILER_BYTES: usize = 512;
const CHECKSUM_KIND_OFFSET: usize = 80;
const CHECKSUM_WIDTH_OFFSET: usize = 84;
const CHECKSUM_CRC_OFFSET: usize = 88;
const XML_OFFSET: usize = 216;
const XML_LENGTH_OFFSET: usize = 224;
const FIRST_RUN_TAG_OFFSET: usize = 204;
const TEST_UNSUPPORTED_CHECKSUM: u32 = 5;
const TEST_UNSUPPORTED_RUN: u32 = 0x8000_fffc;

#[test]
fn inspection_and_quick_verification_leave_the_payload_unread() {
	struct MetadataOnly;
	impl ReadAt for MetadataOnly {
		fn len(&self) -> u64 {
			UDZO.len() as u64
		}

		fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
			assert!(offset >= NATIVE_UDZO_FORK_BYTES, "unexpected payload read");
			UDZO.read_exact_at(offset, buffer)
		}
	}
	let image = Image::open(MetadataOnly, Limits::default()).unwrap();
	let report = inspect(&image, RunWindow::new(0, 1));
	assert_eq!(report.schema_version, SCHEMA_VERSION);
	assert_eq!(report.container.logical_bytes, NATIVE_LOGICAL_BYTES);
	assert_eq!(report.coverage.structure, CheckStatus::NotChecked);
	assert_eq!(report.coverage.payload, CheckStatus::NotChecked);
	assert_eq!(report.run_page.unwrap().runs[0].encoding, RunEncoding::Zlib);

	let quick = verify(MetadataOnly, Limits::default(), VerificationMode::Quick);
	assert!(quick.passed());
	assert_eq!(quick.coverage.structure, CheckStatus::Passed);
	assert_eq!(quick.coverage.payload, CheckStatus::NotChecked);
	assert_eq!(quick.checksums.unwrap().data_fork, CheckStatus::NotChecked);
}

#[test]
fn scope_controls_the_verdict_and_leaves_unchecked_layers_explicit() {
	let container = verify(UDRO, Limits::default(), VerificationMode::Container);
	assert!(container.passed());
	assert_eq!(container.coverage.payload, CheckStatus::Passed);
	assert_eq!(container.coverage.filesystem, CheckStatus::NotChecked);
	assert_eq!(container.coverage.security, CheckStatus::NotChecked);
	assert_eq!(container.checksums.unwrap().tables, [CheckStatus::Passed]);

	let full = verify(UDRO, Limits::default(), VerificationMode::Full);
	assert!(!full.passed());
	assert_eq!(full.outcome, VerificationOutcome::Partial);
	assert_eq!(full.coverage.payload, CheckStatus::Passed);
	assert_eq!(full.coverage.filesystem, CheckStatus::Unavailable);
	let diagnostic = full.diagnostic.unwrap();
	assert_eq!(diagnostic.stage, Stage::Filesystem);
	assert_eq!(diagnostic.category, ErrorCategory::FeatureNotCompiled);
}

#[test]
fn absent_unsupported_and_corrupt_checksums_have_distinct_outcomes() {
	let mut absent = UDRO.to_vec();
	let trailer = absent.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	trailer[CHECKSUM_KIND_OFFSET..CHECKSUM_CRC_OFFSET].fill(0);
	let report = verify(
		absent.as_slice(),
		Limits::default(),
		VerificationMode::Container,
	);
	assert!(report.passed());
	assert_eq!(report.checksums.unwrap().data_fork, CheckStatus::NotPresent);
	assert_eq!(
		report.container.unwrap().data_fork_checksum,
		ChecksumDeclaration::Absent
	);

	let mut unsupported = UDRO.to_vec();
	let trailer = unsupported.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	trailer[CHECKSUM_KIND_OFFSET..CHECKSUM_WIDTH_OFFSET]
		.copy_from_slice(&TEST_UNSUPPORTED_CHECKSUM.to_be_bytes());
	let report = verify(
		unsupported.as_slice(),
		Limits::default(),
		VerificationMode::Container,
	);
	assert_eq!(report.outcome, VerificationOutcome::Partial);
	assert_eq!(report.coverage.payload, CheckStatus::Unsupported);
	let checksums = report.checksums.unwrap();
	assert_eq!(checksums.data_fork, CheckStatus::Unsupported);
	assert_eq!(checksums.master, CheckStatus::NotChecked);

	let mut corrupt = UDRO.to_vec();
	corrupt.last_chunk_mut::<TRAILER_BYTES>().unwrap()[CHECKSUM_CRC_OFFSET] ^= 1;
	let report = verify(
		corrupt.as_slice(),
		Limits::default(),
		VerificationMode::Container,
	);
	assert_eq!(report.outcome, VerificationOutcome::Failed);
	assert_eq!(report.coverage.payload, CheckStatus::Failed);
	assert!(matches!(
		report.cause(),
		Some(Error::ChecksumMismatch { .. })
	));
	let diagnostic = report.diagnostic.unwrap();
	assert_eq!(diagnostic.category, ErrorCategory::VerificationFailed);
	assert!(matches!(
		diagnostic.detail,
		Some(DiagnosticDetail::ChecksumMismatch { .. })
	));
}

#[test]
fn unsupported_run_code_remains_visible_without_a_successful_payload_verdict() {
	let fixture = with_unsupported_run();
	let image = Image::open(fixture.as_slice(), Limits::default()).unwrap();
	let report = inspect(&image, RunWindow::new(0, 1));
	assert_eq!(report.container.runs.unsupported, 1);
	assert_eq!(report.container.runs.decode_unavailable, 1);
	let run = &report.run_page.unwrap().runs[0];
	assert!(!run.decoder_available);
	assert_eq!(
		run.encoding,
		RunEncoding::Unsupported {
			code: TEST_UNSUPPORTED_RUN
		}
	);

	let quick = verify(
		fixture.as_slice(),
		Limits::default(),
		VerificationMode::Quick,
	);
	assert!(quick.passed());
	let container = verify(
		fixture.as_slice(),
		Limits::default(),
		VerificationMode::Container,
	);
	assert!(!container.passed());
	assert_eq!(container.coverage.payload, CheckStatus::Unsupported);
	let details = container.diagnostic.unwrap().detail.unwrap();
	assert_eq!(
		details,
		DiagnosticDetail::UnsupportedRun {
			code: TEST_UNSUPPORTED_RUN,
			logical_sector: 0
		}
	);
	assert_eq!(
		serde_json::to_value(details).unwrap()["logical_sector"],
		"0"
	);
}

fn with_unsupported_run() -> Vec<u8> {
	const RESOURCE_FORK_KEY: &str = "resource-fork";
	const TABLES_KEY: &str = "blkx";
	const DATA_KEY: &str = "Data";
	let mut trailer = *UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	let xml_offset = u64::from_be_bytes(trailer[XML_OFFSET..XML_LENGTH_OFFSET].try_into().unwrap());
	let xml_offset = usize::try_from(xml_offset).unwrap();
	let body = UDRO.len() - TRAILER_BYTES;
	let mut plist = Value::from_reader_xml(&UDRO[xml_offset..body]).unwrap();
	let table = &mut plist
		.as_dictionary_mut()
		.unwrap()
		.get_mut(RESOURCE_FORK_KEY)
		.unwrap()
		.as_dictionary_mut()
		.unwrap()
		.get_mut(TABLES_KEY)
		.unwrap()
		.as_array_mut()
		.unwrap()[0];
	let Value::Data(records) = table
		.as_dictionary_mut()
		.unwrap()
		.get_mut(DATA_KEY)
		.unwrap()
	else {
		panic!("missing fixture block map data");
	};
	records[FIRST_RUN_TAG_OFFSET..][..size_of::<u32>()]
		.copy_from_slice(&TEST_UNSUPPORTED_RUN.to_be_bytes());
	let mut image = UDRO[..xml_offset].to_vec();
	plist.to_writer_xml(&mut image).unwrap();
	let xml_len = (image.len() - xml_offset) as u64;
	trailer[XML_LENGTH_OFFSET..][..size_of::<u64>()].copy_from_slice(&xml_len.to_be_bytes());
	image.extend_from_slice(&trailer);
	image
}

#[test]
fn paging_is_bounded_and_preserves_logical_order() {
	let image = Image::open(HYBRID_UDZO, Limits::default()).unwrap();
	assert!(RunWindow::new(0, 0).is_none());
	assert!(RunWindow::new(0, MAX_RUN_PAGE_SIZE + 1).is_none());
	let first = inspect(&image, RunWindow::new(0, 1)).run_page.unwrap();
	assert_eq!(first.runs.len(), 1);
	let next = first.next_offset.unwrap();
	assert_eq!(next, 1);
	let second = inspect(&image, RunWindow::new(next, 1)).run_page.unwrap();
	assert_eq!(second.runs.len(), 1);
	let first = first.runs[0].logical;
	assert_eq!(
		first.offset_bytes + first.length_bytes,
		second.runs[0].logical.offset_bytes
	);
	let end = inspect(&image, RunWindow::new(usize::MAX, 1))
		.run_page
		.unwrap();
	assert!(end.runs.is_empty());
	assert!(end.next_offset.is_none());
}

#[test]
fn large_physical_lengths_survive_json_without_rounding() {
	const LARGE_IMAGE_BYTES: u64 = (1 << 53) + 1;
	struct SparseImage;
	impl ReadAt for SparseImage {
		fn len(&self) -> u64 {
			LARGE_IMAGE_BYTES
		}

		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			if offset == LARGE_IMAGE_BYTES - TRAILER_BYTES as u64 {
				output.copy_from_slice(UDRO.last_chunk::<TRAILER_BYTES>().unwrap());
				Ok(())
			} else {
				UDRO.read_exact_at(offset, output)
			}
		}
	}
	let image = Image::open(
		SparseImage,
		Limits {
			max_physical_bytes: LARGE_IMAGE_BYTES,
			..Limits::default()
		},
	)
	.unwrap();
	let document = serde_json::to_value(inspect(&image, None)).unwrap();
	assert_eq!(document["schema_version"], SCHEMA_VERSION);
	assert_eq!(document["operation"], "inspect");
	assert_eq!(document["capabilities"]["filesystem"], false);
	assert_eq!(
		document["container"]["physical_bytes"],
		LARGE_IMAGE_BYTES.to_string()
	);
	assert_eq!(
		document["container"]["data_fork"]["length_bytes"],
		NATIVE_LOGICAL_BYTES.to_string()
	);
	assert_eq!(document["container"]["format"], "udif_v4");
	assert_eq!(
		document["container"]["segment_id"].as_str().unwrap().len(),
		32
	);
}

#[test]
fn failed_admission_has_a_versioned_report_without_host_error_text() {
	const PRIVATE_IO_TEXT: &str = "private-host-path\n";
	struct SourceFailure;
	impl ReadAt for SourceFailure {
		fn len(&self) -> u64 {
			UDRO.len() as u64
		}

		fn read_exact_at(&self, _: u64, _: &mut [u8]) -> Result<(), Error> {
			Err(io::Error::new(io::ErrorKind::PermissionDenied, PRIVATE_IO_TEXT).into())
		}
	}
	let denied = verify(
		SourceFailure,
		Limits::default(),
		VerificationMode::Container,
	);
	assert_eq!(denied.outcome, VerificationOutcome::Unavailable);
	assert!(denied.container.is_none());
	let serialized = serde_json::to_string(&denied).unwrap();
	assert!(!serialized.contains(PRIVATE_IO_TEXT.trim_end()));
	assert_eq!(
		denied.diagnostic.unwrap().category,
		ErrorCategory::PermissionDenied
	);

	let invalid = verify(&[][..], Limits::default(), VerificationMode::Quick);
	assert_eq!(invalid.outcome, VerificationOutcome::Failed);
	assert_eq!(invalid.coverage.structure, CheckStatus::Failed);
	assert_eq!(
		invalid.diagnostic.unwrap().category,
		ErrorCategory::InvalidFormat
	);
}

#[test]
fn compiled_decoder_availability_agrees_with_verification() {
	for (fixture, encoding) in [
		(UDZO, RunEncoding::Zlib),
		(ULFO, RunEncoding::Lzfse),
		(ULMO, RunEncoding::Lzma2),
	] {
		let image = Image::open(fixture, Limits::default()).unwrap();
		let inspection = inspect(&image, RunWindow::new(0, MAX_RUN_PAGE_SIZE));
		let page = inspection.run_page.unwrap();
		let matching = page.runs.iter().filter(|run| run.encoding == encoding);
		assert!(matching.clone().next().is_some());
		let unavailable = matching.filter(|run| !run.decoder_available).count();
		if encoding == RunEncoding::Lzma2 {
			assert!(inspection.container.runs.lzma2 > 0);
			assert_eq!(inspection.container.runs.unsupported, 0);
		}
		let verification = verify(fixture, Limits::default(), VerificationMode::Container);
		assert_eq!(verification.passed(), unavailable == 0);
		assert_eq!(inspection.container.runs.decode_unavailable, unavailable);
		if unavailable != 0 {
			assert_eq!(
				verification.diagnostic.unwrap().category,
				ErrorCategory::FeatureNotCompiled
			);
		}
	}
}
