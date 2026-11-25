use std::{
	cell::Cell,
	io::{self, Cursor, Read, Seek, SeekFrom},
};

use flate2::bufread::ZlibDecoder;
use tiff::{
	TiffError,
	decoder::Decoder,
	tags::{CompressionMethod, Tag},
};

use super::{CLASSIC_HEADER_BYTES, Error, ExecutionProfile, check_cancelled, tiff_error};

pub(super) type StripSpan = Cell<Option<(u64, u64)>>;
pub(super) type ImageDecoder<'a> = Decoder<StripSource<'a>>;

/// Restricts a codec read to the declared strip extent while decoding pixels.
/// The TIFF crate otherwise gives its raw and Deflate readers unrestricted input.
pub(super) struct StripSource<'a> {
	cursor: Cursor<&'a [u8]>,
	span: &'a StripSpan,
}

impl<'a> StripSource<'a> {
	pub(super) const fn new(bytes: &'a [u8], span: &'a StripSpan) -> Self {
		Self {
			cursor: Cursor::new(bytes),
			span,
		}
	}
}

impl Read for StripSource<'_> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let Some((start, end)) = self.span.get() else {
			return self.cursor.read(buffer);
		};
		let position = self.cursor.position();
		if !(start..end).contains(&position) {
			return Ok(0);
		}
		let remaining = usize::try_from(end - position).unwrap_or(usize::MAX);
		let length = remaining.min(buffer.len());
		self.cursor.read(&mut buffer[..length])
	}
}

impl Seek for StripSource<'_> {
	fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
		self.cursor.seek(position)
	}
}

pub(super) fn validate_strips(
	decoder: &mut ImageDecoder<'_>,
	span: &StripSpan,
	file: &[u8],
	raster_limit: usize,
	profile: &ExecutionProfile,
) -> Result<(), Error> {
	let compression = decoder
		.find_tag_unsigned(Tag::Compression)
		.map_err(tiff_error)?
		.and_then(CompressionMethod::from_u16)
		.unwrap_or(CompressionMethod::None);
	let offsets = decoder
		.get_tag_u64_vec(Tag::StripOffsets)
		.map_err(tiff_error)?;
	let sizes = decoder
		.get_tag_u64_vec(Tag::StripByteCounts)
		.map_err(tiff_error)?;
	let strips = decoder.strip_count().map_err(tiff_error)?;
	if offsets.len() != sizes.len() || offsets.len() != strips as usize {
		return Err(Error::BackgroundTiffRepresentation);
	}

	let mut raster = Vec::new();
	for (index, (&offset, &size)) in (0..strips).zip(offsets.iter().zip(&sizes)) {
		check_cancelled(profile)?;
		let end = offset
			.checked_add(size)
			.ok_or(Error::BackgroundTiffRepresentation)?;
		let start_index = usize::try_from(offset)
			.ok()
			.filter(|&index| index >= CLASSIC_HEADER_BYTES)
			.ok_or(Error::BackgroundTiffRepresentation)?;
		let end_index = usize::try_from(end).map_err(|_| Error::BackgroundTiffRepresentation)?;
		let encoded = file
			.get(start_index..end_index)
			.ok_or(Error::BackgroundTiffRepresentation)?;
		let raster_size = decoder
			.image_chunk_buffer_layout(index)
			.map_err(tiff_error)?
			.len;
		if raster_size > raster_limit {
			return Err(Error::BackgroundMemory);
		}
		if encoded.is_empty()
			|| (compression == CompressionMethod::None && encoded.len() != raster_size)
		{
			return Err(Error::BackgroundTiffRepresentation);
		}
		if matches!(
			compression,
			CompressionMethod::Deflate | CompressionMethod::OldDeflate
		) {
			validate_deflate(encoded, raster_size)?;
		}
		if raster_size > raster.len() {
			raster.try_reserve_exact(raster_size - raster.len())?;
			if raster.capacity() > raster_limit {
				return Err(Error::BackgroundMemory);
			}
			raster.resize(raster_size, 0);
		}
		span.set(Some((offset, end)));
		let result = decoder.read_chunk_bytes(index, &mut raster[..raster_size]);
		span.set(None);
		result.map_err(tiff_error)?;
	}
	Ok(())
}

fn validate_deflate(encoded: &[u8], raster_size: usize) -> Result<(), Error> {
	// Even an incomplete zlib stream can produce every pixel before the
	// missing trailer is noticed. Read through the end with a one-byte
	// allowance so extra output and incomplete trailers are both rejected.
	let expected = raster_size as u64;
	let mut stream = ZlibDecoder::new(encoded);
	let decoded = io::copy(&mut stream.by_ref().take(expected + 1), &mut io::sink())
		.map_err(|error| tiff_error(TiffError::IoError(error)))?;
	if decoded != expected || stream.total_in() != encoded.len() as u64 {
		return Err(Error::BackgroundTiffRepresentation);
	}
	Ok(())
}
