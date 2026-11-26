#![cfg(all(feature = "read", feature = "zlib"))]

use std::cell::Cell;

use dmgez_udif::{
	Error, MAX_EMBEDDED_SIGNATURE_BYTES, TRAILER_BYTES, embedded_code_directory,
	reader::{CheckStatus, Image, Limits, ReadAt},
	signed_signature_range,
};

const UNSIGNED: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNED: &[u8] =
	include_bytes!("../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const SIGNATURE_LENGTH_FIELD: usize = 304;
const CODE_LIMIT_FIELD: usize = 32;

#[test]
fn verifies_signed_geometry_without_widening_ordinary_open() {
	assert!(matches!(
		Image::open(SIGNED, Limits::default()),
		Err(Error::Unsupported(_))
	));
	let unsigned = Image::open(UNSIGNED, Limits::default()).unwrap();
	let mut signed = Image::open_release(SIGNED, Limits::default()).unwrap();
	assert_eq!(signed.descriptor().physical_bytes, SIGNED.len() as u64);
	assert_eq!(
		signed.descriptor().logical_bytes,
		unsigned.descriptor().logical_bytes
	);
	assert_eq!(
		signed.descriptor().data_fork,
		unsigned.descriptor().data_fork
	);
	assert_eq!(
		signed.descriptor().resource_xml,
		unsigned.descriptor().resource_xml
	);
	let checks = signed.verify().unwrap();
	assert_eq!(checks.data_fork, CheckStatus::Passed);
	assert_eq!(checks.master, CheckStatus::Passed);
	assert_eq!(checks.tables, [CheckStatus::Passed]);
}

#[test]
fn refuses_an_invalid_signed_frame_and_detects_changed_payload_bytes() {
	let mut invalid = SIGNED.to_vec();
	let range = signed_signature_range(SIGNED.last_chunk().unwrap(), SIGNED.len() as u64).unwrap();
	invalid[range.start as usize] ^= 1;
	assert!(matches!(
		Image::open_release(invalid.as_slice(), Limits::default()),
		Err(Error::Corrupt(_))
	));

	let mut corrupt = SIGNED.to_vec();
	corrupt[0] ^= 1;
	let mut image = Image::open_release(corrupt.as_slice(), Limits::default()).unwrap();
	assert!(matches!(
		image.verify(),
		Err(Error::ChecksumMismatch { .. })
	));
}

#[test]
fn rejects_a_well_formed_code_directory_with_the_wrong_body_length() {
	let trailer = SIGNED.last_chunk::<TRAILER_BYTES>().unwrap();
	let range = signed_signature_range(trailer, SIGNED.len() as u64).unwrap();
	let frame = &SIGNED[range.start as usize..range.end as usize];
	let directory = embedded_code_directory(frame).unwrap();
	assert_eq!(directory.signing_limit, range.start);
	let directory_offset = directory.directory.as_ptr() as usize - SIGNED.as_ptr() as usize;
	let limit_offset = directory_offset + CODE_LIMIT_FIELD;
	let incorrect_limit = u32::try_from(range.start - 1).unwrap();
	let mut invalid = SIGNED.to_vec();
	invalid[limit_offset..][..size_of::<u32>()].copy_from_slice(&incorrect_limit.to_be_bytes());
	let frame = &invalid[range.start as usize..range.end as usize];
	assert_eq!(
		embedded_code_directory(frame).unwrap().signing_limit,
		u64::from(incorrect_limit)
	);
	assert!(matches!(
		Image::open_release(invalid.as_slice(), Limits::default()),
		Err(Error::Corrupt(
			"CodeDirectory length differs from the UDIF body length"
		))
	));
}

#[cfg(feature = "write")]
#[test]
fn admits_signed_layout_only_for_explicit_release_rebuild() {
	let unsigned = Image::open(UNSIGNED, Limits::default()).unwrap();
	let signed = Image::open_release(SIGNED, Limits::default()).unwrap();
	assert_eq!(unsigned.embedded_signature_range(), None);
	assert_eq!(
		signed.embedded_signature_range(),
		Some(signed_signature_range(SIGNED.last_chunk().unwrap(), SIGNED.len() as u64).unwrap())
	);
	assert!(
		signed
			.rebuild_release_metadata()
			.unwrap()
			.matches_preserved_fields(&unsigned.rebuild_metadata().unwrap())
	);
	assert!(matches!(
		signed.rebuild_metadata(),
		Err(Error::Unsupported(_))
	));
}

#[cfg(feature = "write")]
#[test]
fn encodes_new_geometry_with_admitted_container_metadata() {
	use dmgez_udif::{
		SECTOR_BYTES,
		writer::{WriteOptions, write_rebuilt_image},
	};

	let source = Image::open_release(SIGNED, Limits::default()).unwrap();
	let original = source.rebuild_release_metadata().unwrap();
	let edited = original.with_logical_bytes(SECTOR_BYTES);
	assert_ne!(source.descriptor().logical_bytes, SECTOR_BYTES);
	let payload = [0x53; SECTOR_BYTES as usize];
	let mut output = Vec::new();
	write_rebuilt_image(
		payload.as_slice(),
		&mut output,
		&WriteOptions::default(),
		edited,
	)
	.unwrap();
	let mut rebuilt = Image::open(output.as_slice(), Limits::default()).unwrap();
	assert!(edited.matches_preserved_fields(&rebuilt.rebuild_metadata().unwrap()));
	assert!(!original.matches_preserved_fields(&edited));
	assert_eq!(rebuilt.verify().unwrap().master, CheckStatus::Passed);
	let mut observed = [0; SECTOR_BYTES as usize];
	rebuilt.read_exact_at(0, &mut observed).unwrap();
	assert_eq!(observed, payload);
}

#[test]
fn bounds_the_signature_frame_before_attempting_a_read() {
	let mut trailer = *SIGNED.last_chunk::<TRAILER_BYTES>().unwrap();
	let range = signed_signature_range(&trailer, SIGNED.len() as u64).unwrap();
	let excessive = MAX_EMBEDDED_SIGNATURE_BYTES as u64 + 1;
	trailer[SIGNATURE_LENGTH_FIELD..][..size_of::<u64>()].copy_from_slice(&excessive.to_be_bytes());
	struct TrailerOnly {
		trailer: [u8; TRAILER_BYTES],
		length: u64,
	}
	impl ReadAt for TrailerOnly {
		fn len(&self) -> u64 {
			self.length
		}
		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			assert_eq!(offset, self.length - TRAILER_BYTES as u64);
			output.copy_from_slice(&self.trailer);
			Ok(())
		}
	}
	let source = TrailerOnly {
		trailer,
		length: range.start + excessive + TRAILER_BYTES as u64,
	};
	assert!(matches!(
		Image::open_release(source, Limits::default()),
		Err(Error::ResourceLimit(_))
	));
}

#[test]
fn applies_physical_and_metadata_budgets_before_reading_declared_ranges() {
	struct Metered<'image> {
		image: &'image [u8],
		bytes_read: Cell<u64>,
	}
	impl ReadAt for &Metered<'_> {
		fn len(&self) -> u64 {
			self.image.len() as u64
		}

		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			self.bytes_read
				.set(self.bytes_read.get() + output.len() as u64);
			self.image.read_exact_at(offset, output)
		}
	}

	for bytes in [UNSIGNED, SIGNED] {
		let baseline = Image::open_release(bytes, Limits::default()).unwrap();
		let xml = &baseline.descriptor().resource_xml;
		let signature = baseline.embedded_signature_range();
		let metadata_bytes = TRAILER_BYTES as u64
			+ (xml.end - xml.start)
			+ signature
				.as_ref()
				.map_or(0, |range| range.end - range.start);
		let source = Metered {
			image: bytes,
			bytes_read: Cell::new(0),
		};
		let physical_bytes = bytes.len() as u64;
		let physical_limit = Limits {
			max_physical_bytes: physical_bytes - 1,
			..Limits::default()
		};
		assert!(matches!(
			Image::open_release_with_metadata_limit(&source, physical_limit, metadata_bytes),
			Err(Error::ResourceLimit("physical image size"))
		));
		assert_eq!(source.bytes_read.get(), 0);
		let exact_physical_limit = Limits {
			max_physical_bytes: physical_bytes,
			..physical_limit
		};
		let frame_only_limit = signature.as_ref().map(|range| {
			(
				TRAILER_BYTES as u64 + range.end - range.start - 1,
				TRAILER_BYTES as u64,
			)
		});
		for (limit, expected_read) in [
			(TRAILER_BYTES as u64 - 1, 0),
			(metadata_bytes - 1, TRAILER_BYTES as u64),
		]
		.into_iter()
		.chain(frame_only_limit)
		{
			source.bytes_read.set(0);
			assert!(matches!(
				Image::open_release_with_metadata_limit(&source, Limits::default(), limit),
				Err(Error::ResourceLimit(_))
			));
			assert_eq!(source.bytes_read.get(), expected_read);
		}
		source.bytes_read.set(0);
		let image =
			Image::open_release_with_metadata_limit(&source, exact_physical_limit, metadata_bytes)
				.unwrap();
		assert_eq!(source.bytes_read.get(), metadata_bytes);
		assert_eq!(image.embedded_signature_range(), signature);
	}
}
