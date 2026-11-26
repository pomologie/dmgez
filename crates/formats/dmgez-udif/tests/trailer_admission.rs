#![cfg(feature = "read")]

use std::cell::Cell;

use dmgez_udif::{
	Error, TRAILER_BYTES, check_trailer_header,
	reader::{Checksum, Image, Limits, ReadAt},
};

const NATIVE_UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
// Independently recorded native fields from tests/fixtures/manifests/tiny-UDRO.json.
const NATIVE_TRAILER_START: usize = 4_832;
const NATIVE_XML_START: usize = 2_048;
const NATIVE_XML_LENGTH: usize = 2_784;
const NATIVE_PHYSICAL_LENGTH: u64 = 5_344;
const NATIVE_LOGICAL_LENGTH: u64 = 2_048;
const NATIVE_VARIANT: u32 = 2;
const NATIVE_TABLE_RECORDS: usize = 2;
const NATIVE_DATA_CRC: u32 = 0x2f96_947b;
const NATIVE_MASTER_CRC: u32 = 0xf271_439c;

const KOLY_MAGIC: &[u8; 4] = b"koly";
const KOLY_HEADER: &[u8; 12] = b"koly\0\0\0\x04\0\0\x02\0";
const HEADER_LENGTH_OFFSET: usize = 8;
const SEGMENT_COUNT_OFFSET: usize = 60;
const XML_OFFSET: usize = 216;
const XML_LENGTH_OFFSET: usize = 224;
const SECTOR_COUNT_OFFSET: usize = 492;
const TRAILER_READ: (u64, usize) = (NATIVE_TRAILER_START as u64, TRAILER_BYTES);

struct ReadSpy<'a> {
	bytes: &'a [u8],
	admitted: &'a [(u64, usize)],
	reads: &'a Cell<usize>,
}

impl ReadAt for ReadSpy<'_> {
	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		let count = self.reads.get();
		assert_eq!(self.admitted.get(count), Some(&(offset, output.len())));
		self.reads.set(count + 1);
		self.bytes.read_exact_at(offset, output)
	}
}

fn assert_refused(bytes: &[u8], admitted: &[(u64, usize)], check: impl FnOnce(Error)) {
	let reads = Cell::new(0);
	let source = ReadSpy {
		bytes,
		admitted,
		reads: &reads,
	};
	let Err(error) = Image::open(source, Limits::default()) else {
		panic!("invalid trailer was admitted");
	};
	check(error);
	assert_eq!(reads.get(), admitted.len());
}

fn replace_field<const N: usize>(bytes: &mut [u8], offset: usize, value: [u8; N]) {
	let footer = bytes.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	footer[offset..offset + N].copy_from_slice(&value);
}

#[test]
fn describes_the_native_unsigned_trailer_without_reading_payload() {
	assert_eq!(TRAILER_BYTES, 512);
	let footer = NATIVE_UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	assert_eq!(&footer[..KOLY_HEADER.len()], KOLY_HEADER);
	check_trailer_header(footer).unwrap();
	let reads = Cell::new(0);
	let admitted = [TRAILER_READ, (NATIVE_XML_START as u64, NATIVE_XML_LENGTH)];
	let image = Image::open(
		ReadSpy {
			bytes: NATIVE_UDRO,
			admitted: &admitted,
			reads: &reads,
		},
		Limits::default(),
	)
	.unwrap();
	let descriptor = image.descriptor();
	assert_eq!(descriptor.physical_bytes, NATIVE_PHYSICAL_LENGTH);
	assert_eq!(descriptor.logical_bytes, NATIVE_LOGICAL_LENGTH);
	assert_eq!(descriptor.data_fork, 0..NATIVE_XML_START as u64);
	assert_eq!(
		descriptor.resource_xml,
		NATIVE_XML_START as u64..NATIVE_TRAILER_START as u64
	);
	assert_eq!(descriptor.variant, NATIVE_VARIANT);
	assert_eq!(descriptor.physical_crc, Checksum::Crc32(NATIVE_DATA_CRC));
	assert_eq!(descriptor.master_crc, Checksum::Crc32(NATIVE_MASTER_CRC));
	assert_eq!(descriptor.tables.len(), 1);
	assert_eq!(descriptor.tables[0].records, NATIVE_TABLE_RECORDS);
	assert_eq!(image.embedded_signature_range(), None);
	assert_eq!(reads.get(), admitted.len());
}

#[test]
fn rejects_bad_header_length_and_counts_before_reading_xml() {
	let mut image = NATIVE_UDRO.to_vec();
	replace_field(
		&mut image,
		HEADER_LENGTH_OFFSET,
		(TRAILER_BYTES as u32 - 1).to_be_bytes(),
	);
	assert_refused(&image, &[TRAILER_READ], |error| {
		assert!(matches!(error, Error::Corrupt("koly header length")));
	});

	image.copy_from_slice(NATIVE_UDRO);
	replace_field(&mut image, SEGMENT_COUNT_OFFSET, 2u32.to_be_bytes());
	assert_refused(&image, &[TRAILER_READ], |error| {
		assert!(matches!(error, Error::Unsupported("segmented disk image")));
	});

	image.copy_from_slice(NATIVE_UDRO);
	replace_field(&mut image, SECTOR_COUNT_OFFSET, 0u64.to_be_bytes());
	assert_refused(&image, &[TRAILER_READ], |error| {
		assert!(matches!(
			error,
			Error::Corrupt("invalid logical disk length")
		));
	});
}

#[test]
fn rejects_every_short_trailer_without_attempting_a_read() {
	let footer = NATIVE_UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	for len in 0..TRAILER_BYTES {
		assert_refused(&footer[..len], &[], |error| {
			assert!(matches!(
				error,
				Error::InvalidFormat("image is shorter than a koly trailer")
			));
		});
	}
}

#[test]
fn does_not_scan_payload_for_koly_magic() {
	let mut image = NATIVE_UDRO.to_vec();
	image[..KOLY_MAGIC.len()].copy_from_slice(KOLY_MAGIC);
	replace_field(&mut image, 0, [0; KOLY_MAGIC.len()]);
	assert_refused(&image, &[TRAILER_READ], |error| {
		assert!(matches!(
			error,
			Error::InvalidFormat("missing koly trailer")
		));
	});
}

#[test]
fn rejects_wrapping_xml_and_xml_crossing_the_footer_before_reading_either_range() {
	let xml_length = 2u64;
	for start in [u64::MAX, NATIVE_TRAILER_START as u64 - 1] {
		let mut image = NATIVE_UDRO.to_vec();
		replace_field(&mut image, XML_OFFSET, start.to_be_bytes());
		replace_field(&mut image, XML_LENGTH_OFFSET, xml_length.to_be_bytes());
		assert_refused(&image, &[TRAILER_READ], |error| {
			assert!(matches!(
				error,
				Error::Corrupt("koly range outside image body")
			));
		});
	}
}
