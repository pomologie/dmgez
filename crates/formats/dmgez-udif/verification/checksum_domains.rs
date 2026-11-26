use std::{
	cell::RefCell,
	io::{self, Read},
	ops::Range,
};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	reader::{CheckStatus, Checksum, Image, Limits, ReadAt, ResourceValue, RunEncoding},
	writer::{Profile, WriteOptions, write_image},
};

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const ZERO: &[u8] = include_bytes!("../../../../tests/fixtures/images/zero-run.dmg");
const IGNORE: &[u8] = include_bytes!("../../../../tests/fixtures/images/ignore-run.dmg");
const COMMENT: &[u8] = include_bytes!("../../../../tests/fixtures/images/comment-run.dmg");
const APM: &[u8] = include_bytes!("../../../../tests/fixtures/images/hybrid-UDZO.dmg");
const FIXTURE_RAW: &[u8] = include_bytes!("../../../../tests/fixtures/expected/tiny.raw");
const APM_TABLE_CRCS: [u32; 4] = [0x3e1b_1535, 0xb39c_4bd9, 0x2e84_1da3, 0];

// Test oracle values are independent of the production format constants and decoder.
const IEEE_REFLECTED_POLYNOMIAL: u32 = 0xedb8_8320;
const KNOWN_VECTOR: &[u8] = b"123456789";
const KNOWN_VECTOR_CRC: u32 = 0xcbf4_3926;
const TRAILER_BYTES: usize = 512;
const DATA_FORK_START_OFFSET: usize = 24;
const DATA_FORK_LENGTH_OFFSET: usize = 32;
const DATA_CHECKSUM_OFFSET: usize = 80;
const DATA_FORK_CRC_OFFSET: usize = 88;
const XML_START_OFFSET: usize = 216;
const XML_LENGTH_OFFSET: usize = 224;
const MASTER_CHECKSUM_OFFSET: usize = 352;
const MASTER_CRC_OFFSET: usize = 360;
const CHECKSUM_DESCRIPTOR_BYTES: usize = 136;
const BLOCK_MAP_CHECKSUM_OFFSET: usize = 64;
const BLOCK_MAP_CRC_OFFSET: usize = 72;
const BLOCK_MAP_COUNT_OFFSET: usize = 200;
const BLOCK_MAP_HEADER_BYTES: usize = 204;
const RUN_RECORD_BYTES: usize = 40;
const RUN_SECTOR_COUNT_OFFSET: usize = 16;
const COMMENT_FIXTURE_RECORDS: u32 = 3;
const UNSUPPORTED_CHECKSUM_KIND: u32 = 17;
const RESOURCE_FORK_KEY: &str = "resource-fork";
const BLOCK_MAPS_KEY: &str = "blkx";
const BLOCK_MAP_DATA_KEY: &str = "Data";
const DATA_CHECKSUM_DOMAIN: &str = "data fork";
const TABLE_ZERO_CHECKSUM_DOMAIN: &str = "block map 0";
const MASTER_CHECKSUM_DOMAIN: &str = "master image";

const SECTOR_SIZE: usize = SECTOR_BYTES as usize;
const SMALL_DISK_SECTORS: usize = 8;
const SMALL_DISK_BYTES: usize = SMALL_DISK_SECTORS * SECTOR_SIZE;
const INPUT_STRIDES: [usize; 4] = [1, SECTOR_SIZE - 1, SECTOR_SIZE + 1, SMALL_DISK_BYTES];
const LARGE_DISK_SECTORS: usize = 257;
const NOISY_SECTOR_SEED: u32 = 0x5cb4_18a7;
const MUTATED_BIT: u8 = 1;

fn reference_crc32(input: &[u8]) -> u32 {
	let mut crc = u32::MAX;
	for &byte in input {
		crc ^= u32::from(byte);
		for _ in 0..u8::BITS {
			let reflected_bit = crc & 1;
			crc >>= 1;
			if reflected_bit != 0 {
				crc ^= IEEE_REFLECTED_POLYNOMIAL;
			}
		}
	}
	!crc
}

fn reference_master_crcs(table_crcs: &[u32]) -> u32 {
	let big_endian: Vec<_> = table_crcs
		.iter()
		.copied()
		.flat_map(u32::to_be_bytes)
		.collect();
	reference_crc32(&big_endian)
}

fn independent_data_range(image: &[u8]) -> Range<usize> {
	let trailer = image.last_chunk::<TRAILER_BYTES>().unwrap();
	let field = |offset: usize| {
		let bytes = *trailer[offset..].first_chunk().unwrap();
		usize::try_from(u64::from_be_bytes(bytes)).unwrap()
	};
	let start = field(DATA_FORK_START_OFFSET);
	let length = field(DATA_FORK_LENGTH_OFFSET);
	start..start.checked_add(length).unwrap()
}

fn trailer_crc(image: &[u8], offset: usize) -> u32 {
	let trailer = image.last_chunk::<TRAILER_BYTES>().unwrap();
	u32::from_be_bytes(*trailer[offset..].first_chunk().unwrap())
}

fn edit_block_map(image: &[u8], edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
	let mut trailer = *image.last_chunk::<TRAILER_BYTES>().unwrap();
	let xml_start = usize::try_from(u64::from_be_bytes(
		*trailer[XML_START_OFFSET..].first_chunk().unwrap(),
	))
	.unwrap();
	let mut resource =
		ResourceValue::from_reader_xml(&image[xml_start..image.len() - TRAILER_BYTES]).unwrap();
	let map = resource.as_dictionary_mut().unwrap()[RESOURCE_FORK_KEY]
		.as_dictionary_mut()
		.unwrap()[BLOCK_MAPS_KEY]
		.as_array_mut()
		.unwrap()[0]
		.as_dictionary_mut()
		.unwrap();
	let ResourceValue::Data(table) = map.get_mut(BLOCK_MAP_DATA_KEY).unwrap() else {
		panic!("native fixture must contain a block map");
	};
	edit(table);
	let mut xml = Vec::new();
	resource.to_writer_xml(&mut xml).unwrap();
	*trailer[XML_LENGTH_OFFSET..].first_chunk_mut().unwrap() =
		u64::try_from(xml.len()).unwrap().to_be_bytes();
	[&image[..xml_start], &xml, &trailer].concat()
}

fn compare_domains(encoded: &[u8], table_crcs: &[u32]) {
	let physical = independent_data_range(encoded);
	let data_crc = reference_crc32(&encoded[physical.clone()]);
	let master_crc = reference_master_crcs(table_crcs);
	assert_eq!(trailer_crc(encoded, DATA_FORK_CRC_OFFSET), data_crc);
	assert_eq!(trailer_crc(encoded, MASTER_CRC_OFFSET), master_crc);

	let mut image = Image::open(encoded, Limits::default()).unwrap();
	let descriptor = image.descriptor();
	assert_eq!(
		descriptor.data_fork,
		physical.start as u64..physical.end as u64
	);
	assert_eq!(descriptor.physical_crc, Checksum::Crc32(data_crc));
	assert_eq!(descriptor.master_crc, Checksum::Crc32(master_crc));
	assert_eq!(descriptor.tables.len(), table_crcs.len());
	for (table, &expected) in descriptor.tables.iter().zip(table_crcs) {
		assert_eq!(table.checksum, Checksum::Crc32(expected));
	}
	let verified = image.verify().unwrap();
	assert_eq!(verified.data_fork, CheckStatus::Passed);
	assert_eq!(verified.tables, vec![CheckStatus::Passed; table_crcs.len()]);
	assert_eq!(verified.master, CheckStatus::Passed);
}

#[test]
fn reference_has_the_known_ieee_crc32_vectors() {
	assert_eq!(reference_crc32(&[]), 0);
	assert_eq!(reference_crc32(KNOWN_VECTOR), KNOWN_VECTOR_CRC);
}

#[test]
fn native_fixtures_confirm_the_exact_zero_ignore_and_master_domains() {
	let full_table_crc = reference_crc32(FIXTURE_RAW);
	for fixture in [UDRO, UDZO, ZERO, COMMENT] {
		compare_domains(fixture, &[full_table_crc]);
	}
	let middle_two_sectors = &FIXTURE_RAW[SECTOR_SIZE..3 * SECTOR_SIZE];
	let ignore_table_crc = reference_crc32(middle_two_sectors);
	assert_ne!(ignore_table_crc, full_table_crc);
	compare_domains(IGNORE, &[ignore_table_crc]);

	let mut reverse_table_crcs = APM_TABLE_CRCS;
	reverse_table_crcs.reverse();
	assert_ne!(
		reference_master_crcs(&reverse_table_crcs),
		reference_master_crcs(&APM_TABLE_CRCS),
	);
	compare_domains(APM, &APM_TABLE_CRCS);
}

#[test]
fn all_zero_streams_have_distinct_physical_and_logical_domains() {
	let raw = [0; 2 * SECTOR_SIZE];
	let table_crc = reference_crc32(&raw);
	assert_ne!(table_crc, reference_crc32(&[]));
	for (profile, physical) in [(Profile::Udro, raw.as_slice()), (Profile::Udzo, &[])] {
		let mut encoded = Vec::new();
		write_image(
			raw.as_slice(),
			raw.len() as u64,
			&mut encoded,
			&WriteOptions {
				profile,
				chunk_bytes: SECTOR_SIZE,
				..WriteOptions::default()
			},
		)
		.unwrap();
		assert_eq!(&encoded[independent_data_range(&encoded)], physical);
		compare_domains(&encoded, &[table_crc]);
	}
}

#[test]
fn absent_and_unsupported_are_distinguished_from_passed_checksums() {
	let mut absent = edit_block_map(UDRO, |table| {
		table[BLOCK_MAP_CHECKSUM_OFFSET..][..CHECKSUM_DESCRIPTOR_BYTES].fill(0);
	});
	let trailer = absent.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	for offset in [DATA_CHECKSUM_OFFSET, MASTER_CHECKSUM_OFFSET] {
		trailer[offset..][..CHECKSUM_DESCRIPTOR_BYTES].fill(0);
	}
	let mut image = Image::open(absent.as_slice(), Limits::default()).unwrap();
	let verified = image.verify().unwrap();
	assert_eq!(verified.data_fork, CheckStatus::NotPresent);
	assert_eq!(verified.tables, [CheckStatus::NotPresent]);
	assert_eq!(verified.master, CheckStatus::NotPresent);

	let mut unsupported = UDRO.to_vec();
	let trailer = unsupported.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	*trailer[DATA_CHECKSUM_OFFSET..].first_chunk_mut().unwrap() =
		UNSUPPORTED_CHECKSUM_KIND.to_be_bytes();
	let mut image = Image::open(unsupported.as_slice(), Limits::default()).unwrap();
	assert_eq!(
		image.descriptor().physical_crc,
		Checksum::Unsupported {
			kind: UNSUPPORTED_CHECKSUM_KIND,
			size_bits: u32::BITS
		}
	);
	assert!(matches!(
		image.verify(),
		Err(Error::Unsupported("checksum algorithm"))
	));
}

#[test]
fn one_bit_changes_are_attributed_to_their_declared_checksum_domain() {
	let data_crc = reference_crc32(&UDRO[independent_data_range(UDRO)]);
	let table_crc = reference_crc32(FIXTURE_RAW);
	let master_crc = reference_master_crcs(&[table_crc]);
	let table = edit_block_map(UDRO, |table| {
		table[BLOCK_MAP_CRC_OFFSET + size_of::<u32>() - 1] ^= MUTATED_BIT;
	});
	let mut data = UDRO.to_vec();
	data.last_chunk_mut::<TRAILER_BYTES>().unwrap()[DATA_FORK_CRC_OFFSET + size_of::<u32>() - 1] ^=
		MUTATED_BIT;
	let mut master = UDRO.to_vec();
	master.last_chunk_mut::<TRAILER_BYTES>().unwrap()[MASTER_CRC_OFFSET + size_of::<u32>() - 1] ^=
		MUTATED_BIT;

	for (encoded, expected_domain, original_crc) in [
		(data, DATA_CHECKSUM_DOMAIN, data_crc),
		(table, TABLE_ZERO_CHECKSUM_DOMAIN, table_crc),
		(master, MASTER_CHECKSUM_DOMAIN, master_crc),
	] {
		let mut image = Image::open(encoded.as_slice(), Limits::default()).unwrap();
		assert!(matches!(
			image.verify(),
			Err(Error::ChecksumMismatch { domain, expected, actual })
				if domain == expected_domain
					&& expected == original_crc ^ u32::from(MUTATED_BIT)
					&& actual == original_crc,
		));
	}
}

struct SegmentedRead<'source> {
	remaining: &'source [u8],
	stride: usize,
}

impl Read for SegmentedRead<'_> {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		let count = output.len().min(self.stride);
		self.remaining.read(&mut output[..count])
	}
}

fn small_mixed_disk() -> [u8; SMALL_DISK_BYTES] {
	let mut bytes = [0; SMALL_DISK_BYTES];
	bytes[..FIXTURE_RAW.len()].copy_from_slice(FIXTURE_RAW);
	let mut state = NOISY_SECTOR_SEED;
	for byte in &mut bytes[4 * SECTOR_SIZE..5 * SECTOR_SIZE] {
		state ^= state << 13;
		state ^= state >> 17;
		state ^= state << 5;
		*byte = state.to_le_bytes()[0];
	}
	bytes
}

#[test]
fn writer_and_reader_match_an_independent_oracle_across_bounded_segments() {
	let logical = small_mixed_disk();
	let expected_table = reference_crc32(&logical);
	let expected_master = reference_master_crcs(&[expected_table]);
	for profile in [Profile::Udro, Profile::Udzo] {
		for sectors_per_run in 1..=SMALL_DISK_SECTORS {
			let options = WriteOptions {
				profile,
				chunk_bytes: sectors_per_run * SECTOR_SIZE,
				..WriteOptions::default()
			};
			for stride in INPUT_STRIDES {
				let source = SegmentedRead {
					remaining: &logical,
					stride,
				};
				let mut encoded = Vec::new();
				let report =
					write_image(source, SMALL_DISK_BYTES as u64, &mut encoded, &options).unwrap();
				assert_eq!(
					report.table_crc, expected_table,
					"{profile:?}, {sectors_per_run}, {stride}"
				);
				assert_eq!(report.master_crc, expected_master);
				assert_eq!(
					report.physical_crc,
					reference_crc32(&encoded[independent_data_range(&encoded)]),
				);
				if profile == Profile::Udzo && sectors_per_run == 1 {
					assert!(report.raw_runs > 0);
					assert!(report.compressed_runs > 0);
					assert!(report.zero_runs > 0);
				}
				compare_domains(&encoded, &[expected_table]);
			}
		}
	}
}

struct ObservedSource<'source> {
	image: &'source [u8],
	data_end: u64,
	payload_requests: &'source RefCell<Vec<Range<u64>>>,
}

impl ReadAt for ObservedSource<'_> {
	fn len(&self) -> u64 {
		self.image.len() as u64
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		if offset < self.data_end && !output.is_empty() {
			self.payload_requests
				.borrow_mut()
				.push(offset..offset + output.len() as u64);
		}
		self.image.read_exact_at(offset, output)
	}
}

#[test]
fn comment_records_cover_no_sectors_and_require_a_terminator() {
	let mut image = Image::open(COMMENT, Limits::default()).unwrap();
	assert_eq!(
		image.descriptor().tables[0].records as u32,
		COMMENT_FIXTURE_RECORDS
	);
	let runs: Vec<_> = image.runs().collect();
	assert_eq!(runs.len(), 1);
	assert_eq!(runs[0].encoding, RunEncoding::Raw);
	assert_eq!(runs[0].logical, 0..FIXTURE_RAW.len() as u64);
	let mut exported = Vec::new();
	image.write_raw_to(&mut exported).unwrap();
	assert_eq!(exported, FIXTURE_RAW);

	let with_nonzero_comment_count = edit_block_map(COMMENT, |table| {
		let count = BLOCK_MAP_HEADER_BYTES + RUN_SECTOR_COUNT_OFFSET;
		*table[count..].first_chunk_mut().unwrap() = 1_u64.to_be_bytes();
	});
	let without_end = edit_block_map(COMMENT, |table| {
		table.truncate(table.len() - RUN_RECORD_BYTES);
		*table[BLOCK_MAP_COUNT_OFFSET..].first_chunk_mut().unwrap() =
			(COMMENT_FIXTURE_RECORDS - 1).to_be_bytes();
	});
	for (image, reason) in [
		(
			with_nonzero_comment_count,
			"comment run describes image data",
		),
		(without_end, "invalid mish terminator"),
	] {
		let payload_requests = RefCell::new(Vec::new());
		let source = ObservedSource {
			image: &image,
			data_end: independent_data_range(&image).end as u64,
			payload_requests: &payload_requests,
		};
		assert!(matches!(
			Image::open(source, Limits::default()),
			Err(Error::Corrupt(actual)) if actual == reason,
		));
		assert!(payload_requests.borrow().is_empty());
	}
}

#[test]
fn crc_verification_streams_across_multiple_physical_and_logical_buffers() {
	let logical_bytes = LARGE_DISK_SECTORS * SECTOR_SIZE;
	let mut logical = FIXTURE_RAW.repeat(logical_bytes / FIXTURE_RAW.len());
	logical.extend_from_slice(&FIXTURE_RAW[..logical_bytes % FIXTURE_RAW.len()]);
	let mut encoded = Vec::new();
	let report = write_image(
		logical.as_slice(),
		logical.len() as u64,
		&mut encoded,
		&WriteOptions {
			profile: Profile::Udro,
			chunk_bytes: logical.len(),
			..WriteOptions::default()
		},
	)
	.unwrap();
	assert_eq!(report.raw_runs, 1);
	compare_domains(&encoded, &[reference_crc32(&logical)]);
	let data_end = independent_data_range(&encoded).end as u64;
	let requests = RefCell::new(Vec::new());
	let mut image = Image::open(
		ObservedSource {
			image: &encoded,
			data_end,
			payload_requests: &requests,
		},
		Limits::default(),
	)
	.unwrap();
	assert!(requests.borrow().is_empty());
	image.verify().unwrap();
	let requests = requests.borrow();
	let mut domains = requests.split_inclusive(|request| request.end == data_end);
	for _ in 0..2 {
		let domain = domains
			.next()
			.expect("physical and logical CRCs both read the raw payload");
		assert!(domain.len() > 1, "one read would not exercise segmentation");
		assert_eq!(domain.first().unwrap().start, 0);
		assert_eq!(domain.last().unwrap().end, data_end);
		assert!(domain.windows(2).all(|pair| pair[0].end == pair[1].start));
	}
	assert!(domains.next().is_none());
}

#[test]
fn a_matching_physical_crc_cannot_mask_a_logical_checksum_mismatch() {
	let mut damaged = UDRO.to_vec();
	let physical = independent_data_range(&damaged);
	assert_eq!(&damaged[physical.clone()], FIXTURE_RAW);
	damaged[physical.start + SECTOR_SIZE] ^= MUTATED_BIT;
	let actual = reference_crc32(&damaged[physical]);
	let expected = reference_crc32(FIXTURE_RAW);
	assert_ne!(expected, actual);
	let trailer = damaged.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	*trailer[DATA_FORK_CRC_OFFSET..].first_chunk_mut().unwrap() = actual.to_be_bytes();

	let mut image = Image::open(damaged.as_slice(), Limits::default()).unwrap();
	assert!(matches!(
		image.verify(),
		Err(Error::ChecksumMismatch { domain, expected: stored, actual: measured })
			if domain == TABLE_ZERO_CHECKSUM_DOMAIN && stored == expected && measured == actual,
	));
}
