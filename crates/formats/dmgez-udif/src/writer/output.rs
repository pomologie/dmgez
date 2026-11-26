use std::io::Write;

use crate::{Error, trailer};

#[cfg(kani)]
#[path = "../../verification/kani/writer_cursor.rs"]
mod writer_cursor_proof;

const TRAILER_BYTES: u128 = trailer::BYTES as u128;

/// Admit the whole payload while reserving room for the fixed trailer.
pub(super) fn write_payload(
	sink: &mut impl Write,
	start: u64,
	limit: u64,
	bytes: &[u8],
) -> Result<u64, Error> {
	let end = end_with_trailer(start, bytes.len() as u64, limit)
		.and_then(|end| u64::try_from(end - TRAILER_BYTES).ok())
		.ok_or(Error::ResourceLimit("physical data fork size"))?;
	sink.write_all(bytes)?;
	Ok(end)
}

/// Admit the complete metadata and trailer before either write.
pub(super) fn image_end(start: u64, xml_len: u64, limit: u64) -> Result<u64, Error> {
	end_with_trailer(start, xml_len, limit)
		.and_then(|end| u64::try_from(end).ok())
		.ok_or(Error::ResourceLimit("physical image size"))
}

fn end_with_trailer(start: u64, bytes: u64, limit: u64) -> Option<u128> {
	// Both variable operands are u64, so the full sum fits in the wider type.
	let end = u128::from(start) + u128::from(bytes) + TRAILER_BYTES;
	(end <= u128::from(limit)).then_some(end)
}
