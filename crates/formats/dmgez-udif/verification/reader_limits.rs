use std::ops::Range;

#[cfg(feature = "zlib")]
use std::io::Write as _;

#[cfg(feature = "zlib")]
use proptest::prelude::*;

use crate::{
	SECTOR_BYTES, TRAILER_BYTES,
	blkx::{self, Kind, NO_RUN_COMMENT, Record},
	resource, trailer,
};

use super::{Error, Image, Limits, ReadAt};

const FIRST_NON_ALLOCATABLE_BYTE_COUNT: u64 = isize::MAX as u64 + 1;
const ONE_SECTOR: u64 = 1;
const ONE_STORED_BYTE: u64 = 1;
const UNKNOWN_RUN_TAG: u32 = 0x8000_0042;

struct MetadataSource {
	tail_offset: u64,
	tail: [u8; TRAILER_BYTES],
	xml_offset: u64,
	xml: Vec<u8>,
}

impl MetadataSource {
	fn new(sectors: u64, xml_offset: u64, xml_len: u64, xml: Vec<u8>) -> Self {
		Self {
			tail_offset: xml_offset + xml_len,
			tail: trailer::encode(
				sectors,
				xml_offset,
				xml_len,
				0,
				0,
				trailer::PARTITION_IMAGE,
				[0; 16],
			),
			xml_offset,
			xml,
		}
	}

	#[cfg(feature = "zlib")]
	fn with_payload(self, mut data: Vec<u8>) -> Vec<u8> {
		assert_eq!(data.len() as u64, self.xml_offset);
		data.extend(self.xml);
		data.extend(self.tail);
		data
	}
}

impl ReadAt for MetadataSource {
	fn len(&self) -> u64 {
		self.tail_offset + TRAILER_BYTES as u64
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		if offset == self.tail_offset && output.len() == self.tail.len() {
			output.copy_from_slice(&self.tail);
		} else {
			assert_eq!(offset, self.xml_offset);
			assert_eq!(output.len(), self.xml.len());
			output.copy_from_slice(&self.xml);
		}
		Ok(())
	}
}

const PERMISSIVE_LIMITS: Limits = Limits {
	max_physical_bytes: u64::MAX,
	max_logical_bytes: u64::MAX,
	max_xml_bytes: usize::MAX,
	max_tables: 1,
	max_records: 2,
	max_compressed_bytes: usize::MAX,
	max_decoded_bytes: usize::MAX,
};

#[test]
fn rejects_xml_larger_than_a_byte_buffer_before_reading_it() {
	let source = MetadataSource::new(ONE_SECTOR, 0, FIRST_NON_ALLOCATABLE_BYTE_COUNT, Vec::new());
	assert!(matches!(
		Image::open(source, PERMISSIVE_LIMITS),
		Err(Error::ResourceLimit("resource XML bytes"))
	));
}

fn run(kind: Kind, sectors: Range<u64>, stored: Range<u64>) -> Record {
	Record {
		kind,
		comment: NO_RUN_COMMENT,
		first_sector: sectors.start,
		sector_count: sectors.end - sectors.start,
		offset: stored.start,
		stored_len: stored.end - stored.start,
	}
}

fn table_source(sectors: u64, stored_len: u64, mut runs: Vec<Record>) -> MetadataSource {
	runs.push(run(Kind::End, sectors..sectors, stored_len..stored_len));
	let table = blkx::encode(sectors, 0, 0, &runs).unwrap();
	let xml = resource::encode(table, false).unwrap();
	MetadataSource::new(sectors, stored_len, xml.len() as u64, xml)
}

fn run_metadata(kind: Kind, sectors: u64, stored_len: u64) -> MetadataSource {
	table_source(
		sectors,
		stored_len,
		vec![run(kind, 0..sectors, 0..stored_len)],
	)
}

#[test]
fn rejects_stored_and_decoded_runs_that_cannot_fit_a_byte_buffer() {
	let stored = run_metadata(Kind::Zlib, ONE_SECTOR, FIRST_NON_ALLOCATABLE_BYTE_COUNT);
	assert!(matches!(
		Image::open(stored, PERMISSIVE_LIMITS),
		Err(Error::ResourceLimit("compressed run size"))
	));
	let sectors = FIRST_NON_ALLOCATABLE_BYTE_COUNT / SECTOR_BYTES;
	let decoded = run_metadata(Kind::Zlib, sectors, ONE_STORED_BYTE);
	assert!(matches!(
		Image::open(decoded, PERMISSIVE_LIMITS),
		Err(Error::ResourceLimit("decoded run size"))
	));
}

#[test]
fn enforces_logical_capacity_and_table_count_without_reading_payload() {
	for (limits, expected) in [
		(
			Limits {
				max_logical_bytes: SECTOR_BYTES - 1,
				..Limits::default()
			},
			"logical disk size",
		),
		(
			Limits {
				max_tables: 0,
				..Limits::default()
			},
			"block-map tables",
		),
	] {
		let source = run_metadata(Kind::Raw, ONE_SECTOR, SECTOR_BYTES);
		assert!(matches!(
			Image::open(source, limits),
			Err(Error::ResourceLimit(detail)) if detail == expected
		));
	}
}

#[test]
fn reads_the_run_index_without_touching_the_payload() {
	let source = run_metadata(Kind::Raw, ONE_SECTOR, SECTOR_BYTES);
	let image = Image::open(source, Limits::default()).unwrap();
	assert_eq!(image.run_ordinal_at(0), Some(0));
	assert_eq!(image.run_ordinal_at(SECTOR_BYTES - 1), Some(0));
	assert_eq!(image.run_ordinal_at(SECTOR_BYTES), None);
	assert_eq!(image.run_ordinal_at(u64::MAX), None);

	let token = crate::Cancellation::default();
	let mut image = image;
	token.cancel();
	assert!(matches!(
		image.verify_with_cancellation(&token),
		Err(Error::Cancelled)
	));
}

#[test]
fn unknown_run_cannot_supply_zero_bytes() {
	let source = run_metadata(
		Kind::Unsupported(UNKNOWN_RUN_TAG),
		ONE_SECTOR,
		ONE_STORED_BYTE,
	);
	let image = Image::open(source, Limits::default()).unwrap();
	let mut output = [u8::MAX];
	assert!(matches!(
		image.read_exact_at(0, &mut output),
		Err(Error::UnsupportedRun {
			kind: UNKNOWN_RUN_TAG,
			sector: 0
		})
	));
	assert_eq!(output, [u8::MAX]);
}

#[test]
fn mixed_run_admission_rejects_logical_overlap_and_physical_reuse() {
	let logical_overlap = table_source(
		2,
		2 * SECTOR_BYTES,
		vec![
			run(Kind::Raw, 0..1, 0..SECTOR_BYTES),
			run(Kind::Raw, 0..1, SECTOR_BYTES..2 * SECTOR_BYTES),
		],
	);
	assert!(matches!(
		Image::open(logical_overlap, Limits::default()),
		Err(Error::Corrupt("misordered, overlapping or empty mish run"))
	));

	let physical_overlap = table_source(
		2,
		SECTOR_BYTES,
		vec![
			run(Kind::Raw, 0..1, 0..SECTOR_BYTES),
			run(Kind::Raw, 1..2, 0..SECTOR_BYTES),
		],
	);
	assert!(matches!(
		Image::open(physical_overlap, Limits::default()),
		Err(Error::Corrupt("mish runs reuse overlapping physical data"))
	));
}

#[cfg(feature = "zlib")]
const SECTOR_SIZE: usize = SECTOR_BYTES as usize;
#[cfg(feature = "zlib")]
const MIXED_LOGICAL_LEN: usize = 3 * SECTOR_SIZE;

#[cfg(feature = "zlib")]
fn zlib_fixture(raw: &[u8]) -> Vec<u8> {
	let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
	encoder.write_all(raw).unwrap();
	encoder.finish().unwrap()
}

#[cfg(feature = "zlib")]
fn mixed_fixture() -> (Vec<u8>, [u8; MIXED_LOGICAL_LEN]) {
	let mut raw = [0; MIXED_LOGICAL_LEN];
	for (index, byte) in raw[..SECTOR_SIZE].iter_mut().enumerate() {
		*byte = index.to_le_bytes()[0];
	}
	raw[SECTOR_SIZE..2 * SECTOR_SIZE].fill(0x5a);
	let mut payload = raw[..SECTOR_SIZE].to_vec();
	payload.extend(zlib_fixture(&raw[SECTOR_SIZE..2 * SECTOR_SIZE]));
	let stored_end = payload.len() as u64;
	let source = table_source(
		3,
		stored_end,
		vec![
			run(Kind::Raw, 0..1, 0..SECTOR_BYTES),
			run(Kind::Zlib, 1..2, SECTOR_BYTES..stored_end),
			run(Kind::Zero, 2..3, stored_end..stored_end),
		],
	);
	(source.with_payload(payload), raw)
}

#[cfg(feature = "zlib")]
#[test]
fn unaligned_read_spans_raw_zlib_and_zero_runs() {
	let (stored, raw) = mixed_fixture();
	let image = Image::open(stored.as_slice(), Limits::default()).unwrap();
	let start = SECTOR_SIZE - 3;
	let end = 2 * SECTOR_SIZE + 5;
	let mut output = vec![u8::MAX; end - start];
	image.read_exact_at(start as u64, &mut output).unwrap();
	assert_eq!(output, raw[start..end]);

	image
		.read_exact_at(MIXED_LOGICAL_LEN as u64, &mut [])
		.unwrap();
	let mut byte = [u8::MAX];
	assert!(matches!(
		image.read_exact_at(MIXED_LOGICAL_LEN as u64, &mut byte),
		Err(Error::OutOfRange { .. })
	));
	assert_eq!(byte, [u8::MAX]);
}

#[cfg(feature = "zlib")]
proptest! {
	#[test]
	fn random_mixed_ranges_match_generated_bytes(
		start in 0..=MIXED_LOGICAL_LEN,
		len in 0..=MIXED_LOGICAL_LEN,
	) {
		let (stored, raw) = mixed_fixture();
		let image = Image::open(stored.as_slice(), Limits::default()).unwrap();
		let mut output = vec![u8::MAX; len];
		let result = image.read_exact_at(start as u64, &mut output);
		if let Some(expected) = raw.get(start..start + len) {
			result.unwrap();
			prop_assert_eq!(output, expected);
		} else {
			prop_assert!(matches!(result, Err(Error::OutOfRange { .. })), "invalid range");
			prop_assert!(output.iter().all(|&byte| byte == u8::MAX));
		}
	}
}

#[cfg(feature = "zlib")]
#[test]
fn rejects_wrong_zlib_output_lengths_and_truncated_input() {
	let too_short = zlib_fixture(&[0x15; SECTOR_SIZE - 1]);
	let too_long = zlib_fixture(&[0x15; SECTOR_SIZE + 1]);
	let mut truncated = zlib_fixture(&[0x15; SECTOR_SIZE]);
	truncated.pop().unwrap();
	for payload in [too_short, too_long, truncated] {
		let image_bytes =
			run_metadata(Kind::Zlib, ONE_SECTOR, payload.len() as u64).with_payload(payload);
		let image = Image::open(image_bytes.as_slice(), Limits::default()).unwrap();
		let mut byte = [u8::MAX];
		assert!(matches!(
			image.read_exact_at(0, &mut byte),
			Err(Error::InvalidZlib {
				offset: 0,
				logical_offset: 0
			})
		));
		assert_eq!(byte, [u8::MAX]);
	}
}

#[cfg(feature = "zlib")]
#[test]
fn bounded_inflate_refuses_large_expansion_in_an_unallocatable_sparse_disk() {
	const EXPANDED_BYTES: usize = 128 * SECTOR_SIZE;
	const SPARSE_SECTORS: u64 = FIRST_NON_ALLOCATABLE_BYTE_COUNT / SECTOR_BYTES;
	let payload = zlib_fixture(&vec![0x15; EXPANDED_BYTES]);
	assert!(payload.len() < SECTOR_SIZE);
	let mut requests = Vec::new();
	assert!(
		crate::codec::inflate_with_buffer(&payload, SECTOR_SIZE, |size| {
			requests.push(size);
			vec![0; size]
		})
		.is_none()
	);
	assert_eq!(
		requests,
		[SECTOR_SIZE + crate::geometry::DECODER_SPARE_BYTES]
	);
	let stored_end = payload.len() as u64;
	let image_bytes = table_source(
		SPARSE_SECTORS,
		stored_end,
		vec![
			run(Kind::Zlib, 0..1, 0..stored_end),
			run(Kind::Zero, 1..SPARSE_SECTORS, stored_end..stored_end),
		],
	)
	.with_payload(payload);
	let image = Image::open(
		image_bytes.as_slice(),
		Limits {
			max_records: 3,
			max_compressed_bytes: SECTOR_SIZE,
			max_decoded_bytes: SECTOR_SIZE,
			..PERMISSIVE_LIMITS
		},
	)
	.unwrap();
	assert_eq!(
		image.descriptor().logical_bytes,
		FIRST_NON_ALLOCATABLE_BYTE_COUNT
	);
	let mut byte = [u8::MAX];
	assert!(matches!(
		image.read_exact_at(0, &mut byte),
		Err(Error::InvalidZlib { .. })
	));
	assert_eq!(byte, [u8::MAX]);
	image
		.read_exact_at(FIRST_NON_ALLOCATABLE_BYTE_COUNT - 1, &mut byte)
		.unwrap();
	assert_eq!(byte, [0]);
}

#[cfg(feature = "zlib")]
#[test]
fn zlib_requires_exact_input_consumption_and_output_length() {
	const NATIVE_UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
	const EXPECTED: &[u8] = include_bytes!("../../../../tests/fixtures/expected/tiny.raw");
	let image = Image::open(NATIVE_UDZO, Limits::default()).unwrap();
	let data = &image.descriptor().data_fork;
	let stored = &NATIVE_UDZO[data.start as usize..data.end as usize];
	let exact_limits = Limits {
		max_compressed_bytes: stored.len(),
		max_decoded_bytes: EXPECTED.len(),
		..Limits::default()
	};
	let exact = Image::open(NATIVE_UDZO, exact_limits).unwrap();
	let mut output = vec![0; EXPECTED.len()];
	exact.read_exact_at(0, &mut output).unwrap();
	assert_eq!(output, EXPECTED);

	assert_eq!(
		crate::codec::inflate(stored, EXPECTED.len()).as_deref(),
		Some(EXPECTED)
	);
	assert!(crate::codec::inflate(stored, EXPECTED.len() - 1).is_none());
	assert!(crate::codec::inflate(stored, EXPECTED.len() + 1).is_none());
	assert!(crate::codec::inflate(&stored[..stored.len() - 1], EXPECTED.len()).is_none());
	let mut trailing = stored.to_vec();
	trailing.push(0);
	assert!(crate::codec::inflate(&trailing, EXPECTED.len()).is_none());
}
