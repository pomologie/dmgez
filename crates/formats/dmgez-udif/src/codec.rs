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
