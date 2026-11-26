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

fn run_metadata(kind: Kind, sectors: u64, stored_len: u64) -> MetadataSource {
	let run = Record {
		kind,
		comment: NO_RUN_COMMENT,
		first_sector: 0,
		sector_count: sectors,
		offset: 0,
		stored_len,
	};
	let terminator = Record {
		kind: Kind::End,
		first_sector: sectors,
		sector_count: 0,
		offset: stored_len,
		stored_len: 0,
		..run
	};
	let table = blkx::encode(sectors, 0, 0, &[run, terminator]).unwrap();
	let xml = resource::encode(table, false).unwrap();
	MetadataSource::new(sectors, stored_len, xml.len() as u64, xml)
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
