#[cfg(feature = "read")]
pub(crate) fn inflate(stored: &[u8], length: usize) -> Option<Vec<u8>> {
	use flate2::{Decompress, FlushDecompress, Status};

	// One extra byte distinguishes an exact stream from one that expands beyond its mapping.
	let mut decoded = vec![0; length.checked_add(1)?];
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
