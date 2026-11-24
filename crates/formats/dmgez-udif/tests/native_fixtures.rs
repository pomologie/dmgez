#![cfg(all(feature = "read", feature = "zlib"))]

use dmgez_udif::{
	Error, SECTOR_BYTES,
	reader::{CheckStatus, Image, Limits, ReadAt},
};
use proptest::prelude::*;

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const ZERO: &[u8] = include_bytes!("../../../../tests/fixtures/images/zero-run.dmg");
const IGNORE: &[u8] = include_bytes!("../../../../tests/fixtures/images/ignore-run.dmg");
const COMMENT: &[u8] = include_bytes!("../../../../tests/fixtures/images/comment-run.dmg");
const APM: &[u8] = include_bytes!("../../../../tests/fixtures/images/hybrid-UDZO.dmg");
const RAW: &[u8] = include_bytes!("../../../../tests/fixtures/expected/tiny.raw");

const TINY_UDZO_DATA_FORK_BYTES: usize = 300;
const APM_TABLE_COUNT: usize = 4;
const APM_LOGICAL_SECTORS: usize = 1800;
const APM_TRAILING_IGNORE_START_SECTOR: usize = 104;

// Trailer offsets from the UDIF wire layout, independent of the production decoder.
const KOLY_BYTES: usize = 512;
const DATA_FORK_CHECKSUM_OFFSET: usize = 80;
const DATA_FORK_CRC_OFFSET: usize = 88;
const XML_OFFSET_FIELD_OFFSET: usize = 216;
const MASTER_CRC_OFFSET: usize = 360;

#[test]
fn verifies_native_and_native_verified_run_profiles() {
	for bytes in [UDRO, UDZO, ZERO, IGNORE, COMMENT] {
		let mut image = Image::open(bytes, Limits::default()).unwrap();
		let verified = image.verify().unwrap();
		assert_eq!(verified.tables, vec![CheckStatus::Passed]);
		assert_eq!(verified.data_fork, CheckStatus::Passed);
		assert_eq!(verified.master, CheckStatus::Passed);
		let mut out = Vec::new();
		image.write_raw_to(&mut out).unwrap();
		assert_eq!(out, RAW);
	}
}

#[test]
fn verifies_native_multiple_tables_and_exports_trailing_ignore_regions() {
	let mut image = Image::open(APM, Limits::default()).unwrap();
	assert_eq!(image.descriptor().tables.len(), APM_TABLE_COUNT);
	let verified = image.verify().unwrap();
	assert_eq!(verified.tables, vec![CheckStatus::Passed; APM_TABLE_COUNT]);
	assert_eq!(verified.master, CheckStatus::Passed);
	let mut out = Vec::new();
	image.write_raw_to(&mut out).unwrap();
	assert_eq!(out.len(), APM_LOGICAL_SECTORS * SECTOR_BYTES as usize);
	assert_eq!(crc32fast::hash(&out), 0xd4f8_1de3);
	assert!(
		out[APM_TRAILING_IGNORE_START_SECTOR * SECTOR_BYTES as usize..]
			.iter()
			.all(|&byte| byte == 0)
	);
}

#[test]
fn metadata_open_never_reads_payload() {
	struct MetadataOnly;
	impl ReadAt for MetadataOnly {
		fn len(&self) -> u64 {
			UDZO.len() as u64
		}
		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			assert!(
				offset >= TINY_UDZO_DATA_FORK_BYTES as u64,
				"open should read only the native trailer and plist"
			);
			UDZO.read_exact_at(offset, output)
		}
	}
	let image = Image::open(MetadataOnly, Limits::default()).unwrap();
	assert_eq!(image.descriptor().logical_bytes, RAW.len() as u64);
}

#[test]
fn checks_distinct_domains_and_reports_missing_checksums() {
	for (offset, domain) in [
		(DATA_FORK_CRC_OFFSET, "data fork"),
		(MASTER_CRC_OFFSET, "master image"),
	] {
		let mut damaged = UDRO.to_vec();
		damaged.last_chunk_mut::<KOLY_BYTES>().unwrap()[offset] ^= 1;
		let mut image = Image::open(damaged.as_slice(), Limits::default()).unwrap();
		assert!(
			matches!(image.verify(), Err(Error::ChecksumMismatch { domain: actual, .. }) if actual == domain)
		);
	}
	let mut no_checksum = UDRO.to_vec();
	let trailer = no_checksum.last_chunk_mut::<KOLY_BYTES>().unwrap();
	trailer[DATA_FORK_CHECKSUM_OFFSET..XML_OFFSET_FIELD_OFFSET].fill(0);
	let mut image = Image::open(no_checksum.as_slice(), Limits::default()).unwrap();
	assert_eq!(image.verify().unwrap().data_fork, CheckStatus::NotPresent);
}

#[test]
fn rejects_truncated_and_overflowing_geometry_and_budget_admission() {
	let options = Limits::default();
	for end in [0, 11, KOLY_BYTES - 1, KOLY_BYTES, UDZO.len() - 1] {
		assert!(Image::open(&UDZO[..end], options).is_err());
	}
	let mut overflow = UDZO.to_vec();
	let trailer = overflow.last_chunk_mut::<KOLY_BYTES>().unwrap();
	let xml_offset_end = XML_OFFSET_FIELD_OFFSET + size_of::<u64>();
	trailer[XML_OFFSET_FIELD_OFFSET..xml_offset_end].copy_from_slice(&u64::MAX.to_be_bytes());
	assert!(matches!(
		Image::open(overflow.as_slice(), options),
		Err(Error::Corrupt(_))
	));
	for limited in [
		Limits {
			max_xml_bytes: 32,
			..options
		},
		Limits {
			max_records: 1,
			..options
		},
		Limits {
			max_decoded_bytes: RAW.len() - 1,
			..options
		},
		Limits {
			max_compressed_bytes: TINY_UDZO_DATA_FORK_BYTES - 1,
			..options
		},
	] {
		assert!(matches!(
			Image::open(UDZO, limited),
			Err(Error::ResourceLimit(_))
		));
	}
}

#[test]
fn rejects_bad_zlib_on_first_read_before_cache() {
	let mut corrupt = UDZO.to_vec();
	corrupt[TINY_UDZO_DATA_FORK_BYTES - 1] ^= 1;
	let mut image = Image::open(corrupt.as_slice(), Limits::default()).unwrap();
	assert!(matches!(
		image.read_exact_at(0, &mut [0; 1]),
		Err(Error::InvalidZlib { .. })
	));
}

proptest! {
	#[test]
	fn random_ranges_match_known_raw(start in 0..=RAW.len(), len in 0..=RAW.len()) {
		for fixture in [UDRO, UDZO, ZERO, IGNORE, COMMENT] {
			let mut image = Image::open(fixture, Limits::default()).unwrap();
			let mut output = vec![0; len];
			let actual = image.read_exact_at(start as u64, &mut output);
			if start + len <= RAW.len() {
				actual.unwrap();
				prop_assert_eq!(&output, &RAW[start..start + len]);
			} else {
				prop_assert!(actual.is_err());
			}
		}
	}
}

#[test]
fn rechecks_admitted_identity_before_serving_decoded_cache() {
	use std::cell::Cell;

	struct Mutable<'state>(&'state Cell<bool>);
	impl ReadAt for Mutable<'_> {
		fn len(&self) -> u64 {
			UDZO.len() as u64
		}
		fn check_unchanged(&self) -> Result<(), Error> {
			if self.0.get() {
				Err(Error::SourceChanged)
			} else {
				Ok(())
			}
		}
		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			self.check_unchanged()?;
			UDZO.read_exact_at(offset, output)
		}
	}
	let mutated = Cell::new(false);
	let mut image = Image::open(Mutable(&mutated), Limits::default()).unwrap();
	image.read_exact_at(0, &mut [0]).unwrap();
	mutated.set(true);
	assert!(matches!(
		image.read_exact_at(0, &mut [0]),
		Err(Error::SourceChanged)
	));
}
