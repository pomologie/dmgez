use std::{
	cell::RefCell,
	io::{self, Read},
	ops::Range,
};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	reader::{CheckStatus, Checksum, Image, Limits, ReadAt},
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
const DATA_FORK_CRC_OFFSET: usize = 88;
const MASTER_CRC_OFFSET: usize = 360;
const TABLE_ZERO_CHECKSUM_DOMAIN: &str = "block map 0";

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
