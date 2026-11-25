use std::ops::Range;

use crate::{Error, TRAILER_BYTES, check_trailer_header};

const SIGNATURE_OFFSET_FIELD: usize = 296;
const SIGNATURE_LENGTH_FIELD: usize = SIGNATURE_OFFSET_FIELD + size_of::<u64>();
const SIGNATURE_FIELDS_END: usize = SIGNATURE_LENGTH_FIELD + size_of::<u64>();
const SIGNATURE_FIELDS: Range<usize> = SIGNATURE_OFFSET_FIELD..SIGNATURE_FIELDS_END;

/// Checks the trailer header and requires both code signature fields to be zero.
///
/// # Errors
///
/// Returns an error for an invalid header or nonzero signature fields.
pub fn validate_unsigned_signature_fields(trailer: &[u8; TRAILER_BYTES]) -> Result<(), Error> {
	check_trailer_header(trailer)?;
	if trailer[SIGNATURE_FIELDS].iter().any(|&byte| byte != 0) {
		return Err(Error::Unsupported("input UDIF has signature fields"));
	}
	Ok(())
}

/// Checks a UDIF v4 signature append and returns the physical signature range.
///
/// Both lengths include their trailing `koly` blocks. The original trailer must
/// have zeroed signature fields. The signed trailer must otherwise match it,
/// with a nonempty signature range that starts at the original trailer offset
/// and ends at the new trailer offset. This describes the append layout
/// produced by native `codesign` on the qualified unsigned UDIF profiles.
///
/// The caller supplies trailers read from the ends of the respective files,
/// checks that the original body bytes are unchanged, and validates the
/// signature through its signing backend.
///
/// # Errors
///
/// Returns an error for invalid trailer headers, a preexisting signature range,
/// changes outside the signature fields, or an inconsistent append range.
pub fn validate_signature_append(
	unsigned_trailer: &[u8; TRAILER_BYTES],
	unsigned_length: u64,
	signed_trailer: &[u8; TRAILER_BYTES],
	signed_length: u64,
) -> Result<Range<u64>, Error> {
	validate_unsigned_signature_fields(unsigned_trailer)?;
	check_trailer_header(signed_trailer)?;
	if unsigned_trailer[..SIGNATURE_OFFSET_FIELD] != signed_trailer[..SIGNATURE_OFFSET_FIELD]
		|| unsigned_trailer[SIGNATURE_FIELDS_END..] != signed_trailer[SIGNATURE_FIELDS_END..]
	{
		return Err(Error::Corrupt(
			"signature append changed other UDIF trailer fields",
		));
	}

	let original_trailer_offset = unsigned_length
		.checked_sub(TRAILER_BYTES as u64)
		.filter(|&offset| offset != 0)
		.ok_or(Error::Corrupt("unsigned UDIF body is missing"))?;
	let signed_trailer_offset = signed_length
		.checked_sub(TRAILER_BYTES as u64)
		.ok_or(Error::Corrupt("signed UDIF trailer is truncated"))?;
	let signature_offset = signature_field(signed_trailer, SIGNATURE_OFFSET_FIELD)?;
	let signature_length = signature_field(signed_trailer, SIGNATURE_LENGTH_FIELD)?;
	let signature_end = signature_offset
		.checked_add(signature_length)
		.ok_or(Error::Corrupt("UDIF signature range overflows"))?;
	if signature_length == 0
		|| signature_offset != original_trailer_offset
		|| signature_end != signed_trailer_offset
	{
		return Err(Error::Corrupt(
			"UDIF signature range does not span the append",
		));
	}
	Ok(signature_offset..signature_end)
}

fn signature_field(trailer: &[u8; TRAILER_BYTES], offset: usize) -> Result<u64, Error> {
	trailer
		.get(offset..)
		.and_then(<[u8]>::first_chunk::<{ size_of::<u64>() }>)
		.copied()
		.map(u64::from_be_bytes)
		.ok_or(Error::Corrupt("truncated UDIF signature trailer field"))
}
