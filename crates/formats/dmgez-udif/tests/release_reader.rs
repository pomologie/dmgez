#![cfg(all(feature = "read", feature = "zlib"))]

use dmgez_udif::{
	Error, MAX_EMBEDDED_SIGNATURE_BYTES, TRAILER_BYTES,
	reader::{CheckStatus, Image, Limits, ReadAt},
	signed_signature_range,
};

const UNSIGNED: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNED: &[u8] =
	include_bytes!("../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const SIGNATURE_LENGTH_FIELD: usize = 304;

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
