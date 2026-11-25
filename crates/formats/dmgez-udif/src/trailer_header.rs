use crate::Error;

/// Size in bytes of the UDIF v4 trailer.
pub const TRAILER_BYTES: usize = 512;

const SIGNATURE: &[u8; 4] = b"koly";
const VERSION: u32 = 4;
const VERSION_OFFSET: usize = 4;
const HEADER_LENGTH_OFFSET: usize = 8;
const HEADER_LENGTH: u32 = TRAILER_BYTES as u32;

/// Checks the magic, version and declared size of the candidate UDIF v4 trailer.
///
/// This check identifies the trailer header. Range, run, checksum and code
/// signature verification require separate checks.
///
/// # Errors
///
/// Returns an error for a missing magic value, unsupported version or incorrect
/// declared trailer size.
pub fn check_trailer_header(bytes: &[u8; TRAILER_BYTES]) -> Result<(), Error> {
	if &bytes[..SIGNATURE.len()] != SIGNATURE {
		return Err(Error::InvalidFormat("missing koly trailer"));
	}
	if bytes[VERSION_OFFSET..][..size_of::<u32>()] != VERSION.to_be_bytes() {
		return Err(Error::Unsupported("koly version"));
	}
	if bytes[HEADER_LENGTH_OFFSET..][..size_of::<u32>()] != HEADER_LENGTH.to_be_bytes() {
		return Err(Error::Corrupt("koly header length"));
	}
	Ok(())
}

#[cfg(feature = "write")]
pub(crate) fn write_trailer_header(bytes: &mut [u8; TRAILER_BYTES]) {
	bytes[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
	crate::binary::put_u32(bytes, VERSION_OFFSET, VERSION);
	crate::binary::put_u32(bytes, HEADER_LENGTH_OFFSET, HEADER_LENGTH);
}
