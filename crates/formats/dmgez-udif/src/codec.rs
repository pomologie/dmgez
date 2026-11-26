#[cfg(feature = "read")]
pub(crate) fn inflate(stored: &[u8], length: usize) -> Option<Vec<u8>> {
	inflate_with_buffer(stored, length, |size| vec![0; size])
}

#[cfg(feature = "read")]
pub(crate) fn inflate_with_buffer(
	stored: &[u8],
	length: usize,
	make_buffer: impl FnOnce(usize) -> Vec<u8>,
) -> Option<Vec<u8>> {
	use flate2::{Decompress, FlushDecompress, Status};

	// One extra byte distinguishes an exact stream from one that expands beyond its mapping.
	let mut decoded = make_buffer(length.checked_add(crate::geometry::DECODER_SPARE_BYTES)?);
	let mut inflater = Decompress::new(true);
	let status = inflater
		.decompress(stored, &mut decoded, FlushDecompress::Finish)
		.ok()?;
	if status != Status::StreamEnd
		|| inflater.total_out() != length as u64
		|| inflater.total_in() != stored.len() as u64
	{
		return None;
	}
	decoded.truncate(length);
	Some(decoded)
}

#[cfg(feature = "write")]
pub(crate) fn deflate(raw: &[u8], level: u8) -> Result<Option<Vec<u8>>, crate::Error> {
	use flate2::{Compress, Compression, FlushCompress, Status};

	// Any stream at least this large is written as raw data.
	let mut encoded = vec![0; raw.len()];
	let mut compressor = Compress::new(Compression::new(level.into()), true);
	let status = compressor
		.compress(raw, &mut encoded, FlushCompress::Finish)
		.map_err(crate::Error::Compression)?;
	let len = usize::try_from(compressor.total_out())
		.map_err(|_| crate::Error::ResourceLimit("encoded run size"))?;
	if status != Status::StreamEnd || len >= raw.len() || compressor.total_in() != raw.len() as u64
	{
		return Ok(None);
	}
	encoded.truncate(len);
	Ok(Some(encoded))
}

#[cfg(feature = "write-cache")]
pub(crate) const CACHE_INFLATE_SCRATCH_BYTES: usize = 16 << 10;

#[cfg(feature = "write-cache")]
pub(crate) fn matches_inflated(mut stored: &[u8], mut raw: &[u8]) -> bool {
	use flate2::{Decompress, FlushDecompress, Status};

	let mut inflater = Decompress::new(true);
	let mut decoded = [0; CACHE_INFLATE_SCRATCH_BYTES];
	loop {
		let input_before = inflater.total_in();
		let output_before = inflater.total_out();
		let Ok(status) = inflater.decompress(stored, &mut decoded, FlushDecompress::None) else {
			return false;
		};
		// Each delta is bounded by its input or scratch slice.
		let consumed = (inflater.total_in() - input_before) as usize;
		let produced = (inflater.total_out() - output_before) as usize;
		let Some((expected, remaining_raw)) = raw.split_at_checked(produced) else {
			return false;
		};
		if decoded[..produced] != *expected {
			return false;
		}
		stored = &stored[consumed..];
		raw = remaining_raw;
		if status == Status::StreamEnd {
			return stored.is_empty() && raw.is_empty();
		}
		if consumed == 0 && produced == 0 {
			return false;
		}
	}
}
