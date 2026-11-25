use std::ops::Range;

use crate::{
	Error, TRAILER_BYTES, check_trailer_header,
	trailer_header::{
		DATA_FORK_LENGTH_OFFSET, DATA_FORK_OFFSET, RESOURCE_FORK_LENGTH_OFFSET,
		RESOURCE_FORK_OFFSET, XML_LENGTH_OFFSET, XML_OFFSET,
	},
};

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
	let range = signed_signature_range(signed_trailer, signed_length)?;
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
	if range.start != original_trailer_offset {
		return Err(Error::Corrupt(
			"UDIF signature range does not span the append",
		));
	}
	Ok(range)
}

/// Returns the embedded signature range immediately before a UDIF v4 trailer.
///
/// This checks the trailer header and physical range, including that the data
/// fork, resource fork and XML range end before the signature. The caller
/// supplies the file length including the trailer and reads the trailer from
/// the file end.
///
/// # Errors
///
/// Returns an error for an invalid header, an absent signature, or an
/// inconsistent range.
pub fn signed_signature_range(
	trailer: &[u8; TRAILER_BYTES],
	length: u64,
) -> Result<Range<u64>, Error> {
	check_trailer_header(trailer)?;
	let trailer_offset = length
		.checked_sub(TRAILER_BYTES as u64)
		.ok_or(Error::Corrupt("signed UDIF trailer is truncated"))?;
	let signature_offset = trailer_word(trailer, SIGNATURE_OFFSET_FIELD)?;
	let signature_length = trailer_word(trailer, SIGNATURE_LENGTH_FIELD)?;
	let signature_end = signature_offset
		.checked_add(signature_length)
		.ok_or(Error::Corrupt("UDIF signature range overflows"))?;
	if signature_offset == 0 || signature_length == 0 || signature_end != trailer_offset {
		return Err(Error::Corrupt(
			"UDIF signature range does not precede the trailer",
		));
	}
	for (offset_field, length_field) in [
		(DATA_FORK_OFFSET, DATA_FORK_LENGTH_OFFSET),
		(RESOURCE_FORK_OFFSET, RESOURCE_FORK_LENGTH_OFFSET),
		(XML_OFFSET, XML_LENGTH_OFFSET),
	] {
		let offset = trailer_word(trailer, offset_field)?;
		let length = trailer_word(trailer, length_field)?;
		if offset
			.checked_add(length)
			.is_none_or(|end| end > signature_offset)
		{
			return Err(Error::Corrupt(
				"UDIF trailer range crosses into the signature",
			));
		}
	}
	Ok(signature_offset..signature_end)
}

/// Checks that stapling changed only the embedded signature length in the UDIF
/// trailer and that the signature area grew without moving its start.
///
/// The caller supplies both trailers and their complete file lengths. Checking
/// the body bytes, signature slots and ticket validity requires separate checks.
///
/// # Errors
///
/// Returns an error for a malformed signed trailer, a change to any other
/// trailer field, or a signature that did not grow.
pub fn validate_staple_layout(
	before: &[u8; TRAILER_BYTES],
	before_length: u64,
	after: &[u8; TRAILER_BYTES],
	after_length: u64,
) -> Result<(), Error> {
	let before_range = signed_signature_range(before, before_length)?;
	let after_range = signed_signature_range(after, after_length)?;
	if before[..SIGNATURE_LENGTH_FIELD] != after[..SIGNATURE_LENGTH_FIELD]
		|| before[SIGNATURE_FIELDS_END..] != after[SIGNATURE_FIELDS_END..]
	{
		return Err(Error::Corrupt("stapling changed other UDIF trailer fields"));
	}
	if after_range.end <= before_range.end {
		return Err(Error::Corrupt("stapled UDIF signature did not grow"));
	}
	Ok(())
}

fn trailer_word(trailer: &[u8; TRAILER_BYTES], offset: usize) -> Result<u64, Error> {
	trailer
		.get(offset..)
		.and_then(<[u8]>::first_chunk::<{ size_of::<u64>() }>)
		.copied()
		.map(u64::from_be_bytes)
		.ok_or(Error::Corrupt("truncated UDIF trailer field"))
}
