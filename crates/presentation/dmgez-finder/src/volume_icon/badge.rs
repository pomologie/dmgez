use std::io::Cursor;

use icns::{IconElement, IconType, Image, OSType, PixelFormat};
use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

use super::{
	Error, FOURCC_BYTES, HEADER_BYTES, ICNS_MAGIC, MAX_BYTES, PNG_MAGIC, PngError,
	encoding::{SLOTS, has_supported_srgb_metadata, validation_memory, write_header},
	kind_and_length, validate, validate_png_chunks,
};

mod artwork;
#[cfg(test)]
mod tests;

/// Maximum encoded size of the source PNG or ICNS for a volume icon badge.
pub const MAX_BADGE_BYTES: usize = 8 << 20;
pub(super) const MAX_BADGE_DIMENSION: u32 = 1024;
const RGBA_CHANNELS: usize = 4;
const OPAQUE: u8 = u8::MAX;
const PNG_DECODER_BYTES: usize = 8 << 20;
const MAX_RASTER_BYTES: usize =
	MAX_BADGE_DIMENSION as usize * MAX_BADGE_DIMENSION as usize * RGBA_CHANNELS;
// Covers the PNG writer's temporary deflate buffers, row scratch and
// the active slot's encoded payload. Slots are rendered one at a time.
const PNG_ENCODER_BYTES: usize = 32 << 20;
// A growing final buffer can retain the old allocation during reallocation.
const ICNS_REALLOCATION_BYTES: usize = MAX_BYTES;

/// Returns the working memory allowance for decoding and compositing a badge.
///
/// The caller must account for the retained source file and final ICNS
/// separately. Returns `None` for encoded input above [`MAX_BADGE_BYTES`].
#[must_use]
pub fn badge_memory(encoded_bytes: usize) -> Option<usize> {
	if encoded_bytes > MAX_BADGE_BYTES {
		return None;
	}
	let composition =
		PNG_DECODER_BYTES + 2 * MAX_RASTER_BYTES + PNG_ENCODER_BYTES + ICNS_REALLOCATION_BYTES;
	Some(composition.max(validation_memory(encoded_bytes)?))
}

/// Composites a local PNG or modern ICNS badge over an original volume glyph.
///
/// PNG sources and PNG representations selected from ICNS must be static,
/// at most 1024 × 1024 pixels and use up to eight-bit sRGB-compatible samples.
/// An ICNS contributes its largest physical representation, with larger
/// logical sizes preferred at equal pixel dimensions. In every destination
/// slot the badge keeps its aspect ratio and blends in the lower right.
///
/// # Errors
/// Rejects malformed or unsupported input, color metadata the renderer cannot
/// preserve, and outputs larger than the supplied budget or the ICNS limit.
pub fn encode_badge(source: &[u8], max_output_bytes: usize) -> Result<Vec<u8>, Error> {
	let mut output = Vec::new();
	reserve_output(&mut output, HEADER_BYTES, max_output_bytes)?;
	let badge = BadgeRaster::decode(source)?;
	write_header(&mut output, ICNS_MAGIC, HEADER_BYTES)?;

	for (_, icon_type) in SLOTS {
		let kind = icon_type.ostype().0;
		let side = icon_type.pixel_width();
		let pixels = artwork::render(&badge, side)?;
		let image = Image::from_data(PixelFormat::RGBA, side, side, pixels)
			.map_err(|source| Error::Encode { kind, source })?;
		let element = IconElement::encode_image_with_type(&image, icon_type)
			.map_err(|source| Error::Encode { kind, source })?;
		let size = HEADER_BYTES
			.checked_add(element.data.len())
			.ok_or(Error::Size)?;
		reserve_output(&mut output, size, max_output_bytes)?;
		write_header(&mut output, kind, size)?;
		output.extend_from_slice(&element.data);
	}

	let size = u32::try_from(output.len()).map_err(|_| Error::Size)?;
	output[FOURCC_BYTES..HEADER_BYTES].copy_from_slice(&size.to_be_bytes());
	Ok(output)
}

fn reserve_output(bytes: &mut Vec<u8>, additional: usize, budget: usize) -> Result<(), Error> {
	let needed = bytes
		.len()
		.checked_add(additional)
		.filter(|&size| size <= MAX_BYTES)
		.ok_or(Error::Size)?;
	if needed > budget {
		return Err(Error::OutputBudget);
	}
	bytes.try_reserve_exact(additional)?;
	if bytes.capacity() > MAX_BYTES {
		return Err(Error::Size);
	}
	if bytes.capacity() > budget {
		return Err(Error::OutputBudget);
	}
	Ok(())
}

struct BadgeRaster {
	width: u32,
	height: u32,
	rgba: Vec<u8>,
}

impl BadgeRaster {
	fn decode(source: &[u8]) -> Result<Self, Error> {
		if source.len() > MAX_BADGE_BYTES {
			return Err(Error::BadgeSize);
		}
		if source.starts_with(&ICNS_MAGIC) {
			return Self::decode_icns(source);
		}
		Self::decode_png(source)
	}

	fn decode_icns(source: &[u8]) -> Result<Self, Error> {
		let admitted = validate(source)?;
		let mut contents = &admitted.as_bytes()[HEADER_BYTES..];
		let mut selected = None;
		while !contents.is_empty() {
			let invalid = || Error::ElementLength {
				offset: source.len() - contents.len(),
			};
			let (kind, size) = kind_and_length(contents).ok_or_else(invalid)?;
			let (element, remaining) = contents.split_at_checked(size).ok_or_else(invalid)?;
			let data = element.get(HEADER_BYTES..).ok_or_else(invalid)?;
			if let Some(icon_type) = IconType::from_ostype(OSType(kind))
				&& selected.is_none_or(|(previous, _)| rank(icon_type) > rank(previous))
			{
				selected = Some((icon_type, data));
			}
			contents = remaining;
		}
		let (icon_type, data) = selected.ok_or(Error::NoImage)?;
		if data.starts_with(&PNG_MAGIC) {
			return Self::decode_png(data);
		}
		let ostype = icon_type.ostype();
		let image = IconElement::new(ostype, data.to_vec())
			.decode_image()
			.map_err(|source| Error::Decode {
				kind: ostype.0,
				source,
			})?;
		let (width, height) = (image.width(), image.height());
		Ok(Self {
			width,
			height,
			rgba: image.into_data().into_vec(),
		})
	}

	fn decode_png(source: &[u8]) -> Result<Self, Error> {
		let chunks = source.strip_prefix(&PNG_MAGIC).ok_or(Error::BadgePng)?;
		validate_png_chunks(chunks, |width, height| {
			(1..=MAX_BADGE_DIMENSION).contains(&width)
				&& (1..=MAX_BADGE_DIMENSION).contains(&height)
		})
		.map_err(|error| match error {
			PngError::Framing | PngError::Checksum => Error::BadgePng,
			PngError::Dimensions => Error::BadgeDimensions,
			PngError::Animation => Error::BadgeAnimation,
		})?;
		if !has_supported_srgb_metadata(chunks) {
			return Err(Error::BadgeColor);
		}

		let mut decoder = Decoder::new(Cursor::new(source));
		decoder.set_limits(Limits {
			bytes: PNG_DECODER_BYTES,
		});
		decoder.set_ignore_text_chunk(true);
		decoder.set_ignore_iccp_chunk(true);
		decoder.set_transformations(Transformations::EXPAND);
		let mut reader = decoder.read_info().map_err(Error::BadgeDecode)?;
		let size = reader
			.output_buffer_size()
			.filter(|&size| size <= MAX_RASTER_BYTES)
			.ok_or(Error::BadgeDimensions)?;
		let mut samples = Vec::new();
		samples.try_reserve_exact(size)?;
		samples.resize(size, 0);
		let output = reader
			.next_frame(&mut samples)
			.map_err(Error::BadgeDecode)?;
		reader.finish().map_err(Error::BadgeDecode)?;
		if output.bit_depth != BitDepth::Eight {
			return Err(Error::BadgeColor);
		}
		let rgba = rgba_pixels(output.color_type, samples)?;
		Ok(Self {
			width: output.width,
			height: output.height,
			rgba,
		})
	}

	fn pixel(&self, x: u32, y: u32) -> [u8; RGBA_CHANNELS] {
		// Admission bounds both dimensions and decoded images have row-major pixels.
		let offset = (y * self.width + x) as usize;
		self.rgba.as_chunks::<RGBA_CHANNELS>().0[offset]
	}
}

fn rank(icon_type: IconType) -> (u32, u32, [u8; FOURCC_BYTES]) {
	let width = icon_type.pixel_width();
	// ic05 denotes 32 standard points in Apple's native icon sets.
	let logical_width = if icon_type == IconType::RGBA32_16x16_2x_ARGB {
		width
	} else {
		icon_type.screen_width()
	};
	(
		width * icon_type.pixel_height(),
		logical_width,
		icon_type.ostype().0,
	)
}

fn rgba_pixels(color: ColorType, samples: Vec<u8>) -> Result<Vec<u8>, Error> {
	if color == ColorType::Rgba {
		return Ok(samples);
	}
	let channels = color.samples();
	let mut rgba = Vec::new();
	rgba.try_reserve_exact(samples.len() / channels * RGBA_CHANNELS)?;
	for sample in samples.chunks_exact(channels) {
		let pixel = match (color, sample) {
			(ColorType::Rgb, &[red, green, blue]) => [red, green, blue, OPAQUE],
			(ColorType::Grayscale, &[gray]) => [gray, gray, gray, OPAQUE],
			(ColorType::GrayscaleAlpha, &[gray, alpha]) => [gray, gray, gray, alpha],
			_ => return Err(Error::BadgeColor),
		};
		rgba.extend_from_slice(&pixel);
	}
	Ok(rgba)
}
