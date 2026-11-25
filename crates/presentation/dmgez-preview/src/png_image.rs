//! Bounded decoding of still PNG rasters for read-only previews.

use std::{
	collections::TryReserveError,
	io::{self, BufRead, Cursor, Read, Seek, SeekFrom},
};

use png::{BitDepth, ColorType, Decoder, DecodingError, Limits, Transformations};
use thiserror::Error;

/// The file signature for a PNG stream.
pub const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
/// Maximum encoded size of a PNG opened as a file preview.
pub const FILE_INPUT_BYTES: usize = 4 << 20;
const FILE_SIDE: usize = 2048;
const FILE_PIXELS: usize = 1 << 20;
const FILE_DECODER_BYTES: usize = 8 << 20;
const INPUT_BATCH_BYTES: usize = 16 << 10;
#[cfg(feature = "png-render")]
pub(crate) const BACKGROUND_INPUT_BYTES: usize = 16 << 20;
#[cfg(feature = "png-render")]
const BACKGROUND_DECODER_BYTES: usize = 16 << 20;
#[cfg(feature = "png-render")]
const BACKGROUND_SIDE: usize = 4096;
#[cfg(feature = "png-render")]
const BACKGROUND_PIXELS: usize = 4 << 20;

/// A bounded PNG decode failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("PNG input exceeds the encoded byte limit")]
	InputBytes,
	#[error("PNG dimensions exceed the raster limits")]
	Dimensions,
	#[error("PNG exceeded the decoder allocation budget")]
	DecoderBudget,
	#[error("PNG format is outside the still-image profile")]
	Profile,
	#[error("PNG has trailing bytes after its end marker")]
	TrailingBytes,
	#[error("PNG decode was cancelled")]
	Cancelled,
	#[error("could not allocate bounded PNG pixels")]
	Allocation(#[from] TryReserveError),
	#[error("PNG could not be decoded")]
	Decode(#[source] DecodingError),
}

#[derive(Clone, Copy, Debug)]
enum Samples {
	Gray,
	GrayAlpha,
	Rgb,
	Rgba,
}

impl Samples {
	const fn count(self) -> usize {
		match self {
			Self::Gray => 1,
			Self::GrayAlpha => 2,
			Self::Rgb => 3,
			Self::Rgba => 4,
		}
	}

	fn rgba(self, pixel: &[u8]) -> [u8; 4] {
		match self {
			Self::Gray => [pixel[0], pixel[0], pixel[0], u8::MAX],
			Self::GrayAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
			Self::Rgb => [pixel[0], pixel[1], pixel[2], u8::MAX],
			Self::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
		}
	}
}

/// A single tightly packed PNG frame with checked dimensions.
#[derive(Debug)]
pub struct Image {
	size: [usize; 2],
	samples: Samples,
	bytes: Vec<u8>,
}

impl Image {
	/// Width and height in pixels.
	#[must_use]
	pub const fn size(&self) -> [usize; 2] {
		self.size
	}

	/// Row-major, unassociated RGBA samples. No color metadata is applied.
	pub fn rgba_pixels(&self) -> impl ExactSizeIterator<Item = [u8; 4]> + '_ {
		self.bytes
			.chunks_exact(self.samples.count())
			.map(|pixel| self.samples.rgba(pixel))
	}

	#[cfg(feature = "png-render")]
	pub(crate) fn rgba_at(&self, x: usize, y: usize) -> [u8; 4] {
		let count = self.samples.count();
		let offset = (y * self.size[0] + x) * count;
		self.samples.rgba(&self.bytes[offset..offset + count])
	}
}

#[derive(Clone, Copy)]
enum Profile {
	File,
	#[cfg(feature = "png-render")]
	Background,
}

struct Budget {
	input_bytes: usize,
	side: usize,
	pixels: usize,
	decoder_bytes: usize,
}

impl Profile {
	const fn budget(self) -> Budget {
		match self {
			Self::File => Budget {
				input_bytes: FILE_INPUT_BYTES,
				side: FILE_SIDE,
				pixels: FILE_PIXELS,
				decoder_bytes: FILE_DECODER_BYTES,
			},
			#[cfg(feature = "png-render")]
			Self::Background => Budget {
				input_bytes: BACKGROUND_INPUT_BYTES,
				side: BACKGROUND_SIDE,
				pixels: BACKGROUND_PIXELS,
				decoder_bytes: BACKGROUND_DECODER_BYTES,
			},
		}
	}
}

/// Decodes one still PNG within the file-preview limits.
///
/// The profile admits 8-bit RGB, RGBA, grayscale, and indexed PNG. Text and ICC
/// data are ignored. `cancelled` is checked while consuming the encoded input.
///
/// # Errors
/// Returns an error for an unsupported profile, incomplete or invalid encoding,
/// trailing bytes, exceeded bounds, allocation failure, or cancellation.
pub fn decode_file(input: &[u8], cancelled: impl Fn() -> bool) -> Result<Image, Error> {
	decode(input, Profile::File, cancelled)
}

#[cfg(feature = "png-render")]
pub(crate) fn decode_background(input: &[u8]) -> Result<Image, Error> {
	decode(input, Profile::Background, || false)
}

fn decode(input: &[u8], profile: Profile, cancelled: impl Fn() -> bool) -> Result<Image, Error> {
	let budget = profile.budget();
	if input.len() > budget.input_bytes {
		return Err(Error::InputBytes);
	}
	let is_file = matches!(profile, Profile::File);
	let mut cursor = Cursor::new(input);
	let mut decoder = Decoder::new_with_limits(
		Input {
			cursor: &mut cursor,
			cancelled: &cancelled,
		},
		Limits {
			bytes: budget.decoder_bytes,
		},
	);
	decoder.set_ignore_text_chunk(true);
	decoder.set_ignore_iccp_chunk(is_file);
	decoder.set_transformations(Transformations::EXPAND);
	let header = decoder.read_header_info().map_err(decode_error)?;
	let source_size = [header.width, header.height];
	let size = [
		usize::try_from(header.width).map_err(|_| Error::Dimensions)?,
		usize::try_from(header.height).map_err(|_| Error::Dimensions)?,
	];
	let pixels = size[0]
		.checked_mul(size[1])
		.filter(|&pixels| (1..=budget.pixels).contains(&pixels))
		.ok_or(Error::Dimensions)?;
	if size.iter().any(|&side| side > budget.side) {
		return Err(Error::Dimensions);
	}
	let mut reader = decoder.read_info().map_err(decode_error)?;
	let info = reader.info();
	if info.animation_control().is_some()
		|| info.bit_depth != BitDepth::Eight
		|| (!is_file && has_custom_color(info))
	{
		return Err(Error::Profile);
	}
	let samples = match reader.output_color_type() {
		(ColorType::Rgb, BitDepth::Eight) => Samples::Rgb,
		(ColorType::Rgba, BitDepth::Eight) => Samples::Rgba,
		(ColorType::Grayscale, BitDepth::Eight) if is_file => Samples::Gray,
		(ColorType::GrayscaleAlpha, BitDepth::Eight) if is_file => Samples::GrayAlpha,
		_ => return Err(Error::Profile),
	};
	let output_bytes = pixels * samples.count();
	if reader.output_buffer_size() != Some(output_bytes) {
		return Err(Error::Dimensions);
	}
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(output_bytes)?;
	bytes.resize(output_bytes, 0);
	let frame = reader.next_frame(&mut bytes).map_err(decode_error)?;
	reader.finish().map_err(decode_error)?;
	drop(reader);
	if [frame.width, frame.height] != source_size || frame.buffer_size() != output_bytes {
		return Err(Error::Profile);
	}
	if is_file && cursor.position() != input.len() as u64 {
		return Err(Error::TrailingBytes);
	}
	if cancelled() {
		return Err(Error::Cancelled);
	}
	Ok(Image {
		size,
		samples,
		bytes,
	})
}

fn has_custom_color(info: &png::Info<'_>) -> bool {
	info.icc_profile.is_some()
		|| info.coding_independent_code_points.is_some()
		|| info.mastering_display_color_volume.is_some()
		|| info.content_light_level.is_some()
		|| (info.srgb.is_none() && (info.gama_chunk.is_some() || info.chrm_chunk.is_some()))
}

fn decode_error(error: DecodingError) -> Error {
	match error {
		DecodingError::LimitsExceeded => Error::DecoderBudget,
		DecodingError::IoError(cause) if cause.kind() == io::ErrorKind::Interrupted => {
			Error::Cancelled
		}
		error => Error::Decode(error),
	}
}

struct Input<'a, 'b, F> {
	cursor: &'a mut Cursor<&'b [u8]>,
	cancelled: &'a F,
}

impl<F: Fn() -> bool> Input<'_, '_, F> {
	fn check(&self) -> io::Result<()> {
		if (self.cancelled)() {
			return Err(io::ErrorKind::Interrupted.into());
		}
		Ok(())
	}
}

impl<F: Fn() -> bool> Read for Input<'_, '_, F> {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		let count = self.fill_buf()?.read(output)?;
		self.consume(count);
		Ok(count)
	}
}

impl<F: Fn() -> bool> BufRead for Input<'_, '_, F> {
	fn fill_buf(&mut self) -> io::Result<&[u8]> {
		self.check()?;
		let remaining = self.cursor.fill_buf()?;
		Ok(&remaining[..remaining.len().min(INPUT_BATCH_BYTES)])
	}

	fn consume(&mut self, amount: usize) {
		self.cursor.consume(amount);
	}
}

impl<F: Fn() -> bool> Seek for Input<'_, '_, F> {
	fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
		self.check()?;
		self.cursor.seek(from)
	}
}

#[cfg(test)]
mod tests;
