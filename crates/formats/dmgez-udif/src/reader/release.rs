use std::{io, ops::Range};

use crate::{
	Error, MAX_EMBEDDED_SIGNATURE_BYTES, TRAILER_BYTES, embedded_code_directory,
	signature_append::clear_signature_fields, signed_signature_range,
	validate_unsigned_signature_fields,
};

use super::{Image, Limits, ReadAt};

impl<S: ReadAt> Image<S> {
	/// Open an unsigned image or a structurally admitted signed release envelope.
	///
	/// A signed or stapled image keeps its full physical length in the descriptor.
	/// Its exact bounded signature frame is admitted before decoding the disk
	/// structure, and none of its bytes can belong to a mapped data range.
	/// Signature integrity, authenticity and ticket validity are separate checks.
	///
	/// # Errors
	/// Rejects invalid signature framing, malformed or unsupported container
	/// metadata, size limit violations, and I/O failures.
	pub fn open_release(source: S, limits: Limits) -> Result<Self, Error> {
		let (mut tail, trailer_offset) = read_trailer(&source, limits)?;
		let mut body_len = trailer_offset;
		if validate_unsigned_signature_fields(&tail).is_err() {
			body_len = admit_signed_body(&source, &tail)?;
			clear_signature_fields(&mut tail);
		}
		Self::open_from_trailer(source, limits, &tail, body_len)
	}

	/// Returns the structurally admitted signature range, if present.
	#[must_use]
	pub fn embedded_signature_range(&self) -> Option<Range<u64>> {
		let trailer = self.descriptor().physical_bytes - TRAILER_BYTES as u64;
		(self.body_len < trailer).then_some(self.body_len..trailer)
	}
}

pub(super) fn read_trailer(
	source: &impl ReadAt,
	limits: Limits,
) -> Result<([u8; TRAILER_BYTES], u64), Error> {
	let len = source.len();
	if len > limits.max_physical_bytes {
		return Err(Error::ResourceLimit("physical image size"));
	}
	let trailer_offset = len
		.checked_sub(TRAILER_BYTES as u64)
		.ok_or(Error::InvalidFormat("image is shorter than a koly trailer"))?;
	let mut tail = [0; TRAILER_BYTES];
	source.read_exact_at(trailer_offset, &mut tail)?;
	Ok((tail, trailer_offset))
}

fn admit_signed_body(source: &impl ReadAt, tail: &[u8; TRAILER_BYTES]) -> Result<u64, Error> {
	let range = signed_signature_range(tail, source.len())?;
	let size = usize::try_from(range.end - range.start)
		.ok()
		.filter(|size| *size <= MAX_EMBEDDED_SIGNATURE_BYTES)
		.ok_or(Error::ResourceLimit("UDIF embedded signature bytes"))?;
	let mut frame = Vec::new();
	frame.try_reserve_exact(size).map_err(io::Error::other)?;
	frame.resize(size, 0);
	source.read_exact_at(range.start, &mut frame)?;
	let directory = embedded_code_directory(&frame)?;
	if directory.signing_limit != range.start {
		return Err(Error::Corrupt(
			"CodeDirectory length differs from the UDIF body length",
		));
	}
	Ok(range.start)
}
