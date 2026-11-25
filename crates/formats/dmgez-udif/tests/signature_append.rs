use dmgez_udif::{
	Error, TRAILER_BYTES, validate_signature_append, validate_unsigned_signature_fields,
};

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNATURE_OFFSET_FIELD: usize = 296;
const SIGNATURE_LENGTH_FIELD: usize = 304;
const SIGNATURE_FIELDS_END: usize = 312;
const SIGNATURE_LENGTH: u64 = 346;

#[test]
fn accepts_a_contiguous_append_to_each_qualified_trailer() {
	for image in [UDRO, UDZO] {
		let unsigned = trailer(image);
		validate_unsigned_signature_fields(unsigned).unwrap();
		let body_length = (image.len() - TRAILER_BYTES) as u64;
		let signed = signature_fields(*unsigned, body_length, SIGNATURE_LENGTH);
		assert_eq!(
			validate_signature_append(
				unsigned,
				image.len() as u64,
				&signed,
				image.len() as u64 + SIGNATURE_LENGTH,
			)
			.unwrap(),
			body_length..body_length + SIGNATURE_LENGTH,
		);
	}
}

#[test]
fn rejects_source_signature_fields_and_any_unrelated_trailer_change() {
	let unsigned = trailer(UDRO);
	let length = UDRO.len() as u64;
	let body_length = length - TRAILER_BYTES as u64;
	let signed = signature_fields(*unsigned, body_length, SIGNATURE_LENGTH);
	for offset in SIGNATURE_OFFSET_FIELD..SIGNATURE_FIELDS_END {
		let mut existing = *unsigned;
		existing[offset] = 1;
		assert!(matches!(
			validate_unsigned_signature_fields(&existing),
			Err(Error::Unsupported(_))
		));
		assert!(matches!(
			validate_signature_append(&existing, length, &signed, length + SIGNATURE_LENGTH),
			Err(Error::Unsupported(_))
		));
	}
	for offset in (0..SIGNATURE_OFFSET_FIELD).chain(SIGNATURE_FIELDS_END..TRAILER_BYTES) {
		let mut changed = signed;
		changed[offset] ^= 1;
		assert!(
			validate_signature_append(unsigned, length, &changed, length + SIGNATURE_LENGTH)
				.is_err(),
			"accepted unrelated change at trailer offset {offset}"
		);
	}
}

#[test]
fn rejects_gaps_overlap_overflow_and_missing_bytes() {
	let unsigned = trailer(UDZO);
	let length = UDZO.len() as u64;
	let body_length = length - TRAILER_BYTES as u64;
	let signed_length = length + SIGNATURE_LENGTH;
	for (offset, size, original, signed) in [
		(body_length, 0, length, length),
		(body_length, SIGNATURE_LENGTH, length, signed_length - 1),
		(body_length, SIGNATURE_LENGTH, length, signed_length + 1),
		(body_length - 1, SIGNATURE_LENGTH + 1, length, signed_length),
		(body_length + 1, SIGNATURE_LENGTH - 1, length, signed_length),
		(u64::MAX, SIGNATURE_LENGTH, length, signed_length),
		(0, SIGNATURE_LENGTH, TRAILER_BYTES as u64, signed_length),
		(0, SIGNATURE_LENGTH, TRAILER_BYTES as u64 - 1, signed_length),
		(
			body_length,
			SIGNATURE_LENGTH,
			length,
			TRAILER_BYTES as u64 - 1,
		),
	] {
		let signed_trailer = signature_fields(*unsigned, offset, size);
		assert!(matches!(
			validate_signature_append(unsigned, original, &signed_trailer, signed),
			Err(Error::Corrupt(_))
		));
	}
}

fn trailer(image: &[u8]) -> &[u8; TRAILER_BYTES] {
	image.last_chunk().unwrap()
}

fn signature_fields(
	mut trailer: [u8; TRAILER_BYTES],
	offset: u64,
	length: u64,
) -> [u8; TRAILER_BYTES] {
	trailer[SIGNATURE_OFFSET_FIELD..SIGNATURE_LENGTH_FIELD].copy_from_slice(&offset.to_be_bytes());
	trailer[SIGNATURE_LENGTH_FIELD..SIGNATURE_FIELDS_END].copy_from_slice(&length.to_be_bytes());
	trailer
}
