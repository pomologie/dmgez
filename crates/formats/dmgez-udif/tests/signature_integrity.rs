#![cfg(feature = "signature-integrity")]

use std::io::{Cursor, Read};

use dmgez_udif::{
	Error, MAX_EMBEDDED_SIGNATURE_BYTES, SignatureIntegrityError, TRAILER_BYTES,
	read_embedded_signature, verify_signature_integrity,
};

// Wire offsets from the native fixture recorded in tiny-UDZO-adhoc-sha256.json.
const NATIVE_SIGNED: &[u8] =
	include_bytes!("../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const CODE_DIRECTORY_OFFSET: usize = 3120;
const CODE_LIMIT_FIELD: usize = 32;
const REQUIREMENTS_OFFSET: usize = 3415;
const REQUIREMENTS_COUNT_FIELD: usize = 8;

#[test]
fn matches_all_three_digests_in_the_native_ad_hoc_signature() {
	verify_signature_integrity(&mut Cursor::new(NATIVE_SIGNED)).unwrap();
}

#[test]
fn rejects_each_changed_digest_input() {
	let last_trailer_byte = NATIVE_SIGNED.len() - 1;
	for (byte, expected) in [
		(0, SignatureIntegrityError::BodyDigest),
		(last_trailer_byte, SignatureIntegrityError::TrailerDigest),
		(
			REQUIREMENTS_OFFSET + REQUIREMENTS_COUNT_FIELD,
			SignatureIntegrityError::RequirementsDigest,
		),
		(
			CODE_DIRECTORY_OFFSET + CODE_LIMIT_FIELD,
			SignatureIntegrityError::BodyLength,
		),
	] {
		let mut image = NATIVE_SIGNED.to_vec();
		image[byte] ^= 1;
		let error = verify_signature_integrity(&mut Cursor::new(image)).unwrap_err();
		assert_eq!(
			std::mem::discriminant(&error),
			std::mem::discriminant(&expected),
			"{error} at {byte}"
		);
	}
}

#[test]
fn rejects_excessive_frames_before_reading_and_detects_short_reads() {
	struct NeverRead;
	impl Read for NeverRead {
		fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
			panic!("oversized signature must fail before reading");
		}
	}
	assert!(matches!(
		read_embedded_signature(&mut NeverRead, MAX_EMBEDDED_SIGNATURE_BYTES as u64 + 1),
		Err(Error::ResourceLimit(_)),
	));
	assert!(matches!(
		verify_signature_integrity(&mut Cursor::new(&NATIVE_SIGNED[..TRAILER_BYTES - 1])),
		Err(SignatureIntegrityError::Format(Error::InvalidFormat(_))),
	));
	assert!(matches!(
		read_embedded_signature(&mut Cursor::new([]), 1),
		Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof,
	));
}
