use std::{
	io::{Cursor, Seek, Write},
	mem::size_of,
};

use png::{BitDepth, ColorType, OutputInfo};
use tiff::{
	Directory,
	encoder::{
		ImageEncoder, TiffEncoder, TiffKindStandard,
		colortype::{ColorType as TiffColor, Gray8, Gray16, RGB8, RGB16},
	},
	tags::{ExtraSamples, Tag, Type},
};

use crate::{preflight::ExecutionProfile, presentation::check_cancelled};

use super::{
	Error, MAX_IMAGE_DIMENSION, MAX_PNG_DECODER_BYTES, MAX_RASTER_BYTES,
	PNG_DECODER_BUDGET_DIVISOR, RETINA_SCALE, icc,
	output::{
		BASE_DPI, CappedOutput, RETINA_DPI, tiff_encode_error as encode_error,
		write_tiff_resolution,
	},
};

mod orientation;
#[cfg(test)]
mod tests;

const TIFF_STRIP_BYTES: usize = 64 << 10;
// This covers the IFD, encoder strip-offset/size tables and their temporary
// tag buffers. The 64 MiB raster limit keeps the strip count below 2,048.
const TIFF_TAG_WORKSPACE_BYTES: usize = 64 << 10;
const PNG_SIGNATURE_BYTES: usize = 8;
const PNG_CHUNK_HEADER_BYTES: usize = 8;
const PNG_CHUNK_CRC_BYTES: usize = 4;

/// Encodes the two representations into one classic, uncompressed TIFF.
/// The output is billed to the metadata cap; decoding and strip conversion
/// are separately billed to the supplied working-memory cap.
pub(super) fn encode(
	base_png: &[u8],
	retina_png: &[u8],
	max_buffer_bytes: usize,
	max_encoded_bytes: usize,
	profile: &ExecutionProfile,
) -> Result<Vec<u8>, Error> {
	check_cancelled(profile)?;
	let decode_bytes = max_buffer_bytes
		.checked_sub(TIFF_TAG_WORKSPACE_BYTES + TIFF_STRIP_BYTES)
		.ok_or(Error::BackgroundMemory)?;
	let mut output = CappedOutput::new(max_encoded_bytes);
	{
		let mut encoder = TiffEncoder::new(&mut output).map_err(encode_error)?;
		let (width, height) = write_page(
			&mut encoder,
			base_png,
			decode_bytes,
			BASE_DPI,
			None,
			profile,
		)?;
		let expected = (
			width
				.checked_mul(RETINA_SCALE)
				.ok_or(Error::BackgroundRetinaDimensions)?,
			height
				.checked_mul(RETINA_SCALE)
				.ok_or(Error::BackgroundRetinaDimensions)?,
		);
		write_page(
			&mut encoder,
			retina_png,
			decode_bytes,
			RETINA_DPI,
			Some(expected),
			profile,
		)?;
	}
	check_cancelled(profile)?;
	Ok(output.into_bytes())
}

fn write_page(
	encoder: &mut TiffEncoder<&mut CappedOutput>,
	bytes: &[u8],
	available: usize,
	dpi: u32,
	expected_dimensions: Option<(u32, u32)>,
	profile: &ExecutionProfile,
) -> Result<(u32, u32), Error> {
	check_cancelled(profile)?;
	let decoder_bytes = (available / PNG_DECODER_BUDGET_DIVISOR).min(MAX_PNG_DECODER_BYTES);
	let raster_bytes = (available - decoder_bytes).min(MAX_RASTER_BYTES);
	let mut decoder = png::Decoder::new_with_limits(
		Cursor::new(bytes),
		png::Limits {
			bytes: decoder_bytes,
		},
	);
	decoder.set_ignore_text_chunk(true);
	decoder.set_transformations(png::Transformations::EXPAND);
	let header = decoder.read_header_info().map_err(decode_error)?;
	let dimensions = (header.width, header.height);
	if dimensions.0 > MAX_IMAGE_DIMENSION || dimensions.1 > MAX_IMAGE_DIMENSION {
		return Err(Error::BackgroundMemory);
	}
	if expected_dimensions.is_some_and(|expected| expected != dimensions) {
		return Err(Error::BackgroundRetinaDimensions);
	}
	let mut reader = decoder.read_info().map_err(decode_error)?;
	if reader.info().animation_control.is_some() {
		return Err(Error::BackgroundAnimated);
	}
	let size = reader
		.output_buffer_size()
		.filter(|&size| size <= raster_bytes)
		.ok_or(Error::BackgroundMemory)?;
	let mut pixels = Vec::new();
	pixels.try_reserve_exact(size)?;
	if pixels.capacity() > raster_bytes {
		return Err(Error::BackgroundMemory);
	}
	pixels.resize(size, 0);
	let image = reader.next_frame(&mut pixels).map_err(decode_error)?;
	reader.finish().map_err(decode_error)?;
	check_cancelled(profile)?;

	let gray = matches!(
		image.color_type,
		ColorType::Grayscale | ColorType::GrayscaleAlpha
	);
	let icc = compatible_icc(reader.info(), bytes, gray)?;
	let page = Page {
		image,
		pixels: &pixels,
		icc,
		dpi,
	};
	match (page.image.color_type, page.image.bit_depth) {
		(ColorType::Grayscale | ColorType::GrayscaleAlpha, BitDepth::Eight) => {
			write_u8::<Gray8>(encoder, &page, profile)?
		}
		(ColorType::Rgb | ColorType::Rgba, BitDepth::Eight) => {
			write_u8::<RGB8>(encoder, &page, profile)?
		}
		(ColorType::Grayscale | ColorType::GrayscaleAlpha, BitDepth::Sixteen) => {
			write_u16::<Gray16>(encoder, &page, profile)?
		}
		(ColorType::Rgb | ColorType::Rgba, BitDepth::Sixteen) => {
			write_u16::<RGB16>(encoder, &page, profile)?
		}
		_ => return Err(Error::BackgroundRetinaPngFormat),
	}
	Ok(dimensions)
}

struct Page<'a> {
	image: OutputInfo,
	pixels: &'a [u8],
	icc: Option<&'a [u8]>,
	dpi: u32,
}

fn start_image<'a, W: Write + Seek, C: TiffColor>(
	encoder: &'a mut TiffEncoder<W>,
	page: &Page<'_>,
) -> Result<ImageEncoder<'a, W, C, TiffKindStandard>, Error> {
	let mut image = encoder
		.new_image::<C>(page.image.width, page.image.height)
		.map_err(encode_error)?;
	if matches!(
		page.image.color_type,
		ColorType::GrayscaleAlpha | ColorType::Rgba
	) {
		image
			.extra_samples(&[ExtraSamples::UnassociatedAlpha])
			.map_err(encode_error)?;
	}
	let rows = u32::try_from((TIFF_STRIP_BYTES / page.image.line_size).max(1))
		.map_err(|_| Error::BackgroundMemory)?;
	image.rows_per_strip(rows).map_err(encode_error)?;

	let tags = image.encoder();
	write_tiff_resolution(tags, page.dpi).map_err(encode_error)?;
	if let Some(icc) = page.icc {
		let entry = tags
			.write_entry_bytes(Type::UNDEFINED, icc)
			.map_err(encode_error)?;
		tags.extend_from(&Directory::from_iter([(Tag::IccProfile, entry)]));
	}
	Ok(image)
}

fn write_u8<C: TiffColor<Inner = u8>>(
	encoder: &mut TiffEncoder<&mut CappedOutput>,
	page: &Page<'_>,
	profile: &ExecutionProfile,
) -> Result<(), Error> {
	let mut image = start_image::<_, C>(encoder, page)?;
	let mut pixels = page.pixels;
	while image.next_strip_sample_count() > 0 {
		check_cancelled(profile)?;
		let samples = usize::try_from(image.next_strip_sample_count())
			.map_err(|_| Error::BackgroundMemory)?;
		let Some((strip, remainder)) = pixels.split_at_checked(samples) else {
			return Err(Error::BackgroundRetinaPngFormat);
		};
		image.write_strip(strip).map_err(encode_error)?;
		pixels = remainder;
	}
	image.finish().map_err(encode_error)
}

fn write_u16<C: TiffColor<Inner = u16>>(
	encoder: &mut TiffEncoder<&mut CappedOutput>,
	page: &Page<'_>,
	profile: &ExecutionProfile,
) -> Result<(), Error> {
	let mut image = start_image::<_, C>(encoder, page)?;
	let mut pixels = page.pixels;
	let mut strip = Vec::new();
	strip.try_reserve_exact(TIFF_STRIP_BYTES / size_of::<u16>())?;
	if strip.capacity() > TIFF_STRIP_BYTES / size_of::<u16>() {
		return Err(Error::BackgroundMemory);
	}
	while image.next_strip_sample_count() > 0 {
		check_cancelled(profile)?;
		let count = usize::try_from(image.next_strip_sample_count())
			.ok()
			.filter(|&count| count <= strip.capacity())
			.ok_or(Error::BackgroundMemory)?;
		let Some((network_samples, remainder)) = pixels.split_at_checked(count * size_of::<u16>())
		else {
			return Err(Error::BackgroundRetinaPngFormat);
		};
		strip.clear();
		strip.extend(
			network_samples
				.as_chunks::<{ size_of::<u16>() }>()
				.0
				.iter()
				.copied()
				.map(u16::from_be_bytes),
		);
		image.write_strip(&strip).map_err(encode_error)?;
		pixels = remainder;
	}
	image.finish().map_err(encode_error)
}

fn compatible_icc<'a>(
	info: &'a png::Info<'_>,
	png: &[u8],
	gray: bool,
) -> Result<Option<&'a [u8]>, Error> {
	// Preserve the orientation and color appearance of the decoded samples.
	if info
		.exif_metadata
		.as_deref()
		.is_some_and(|exif| !orientation::is_top_left(exif))
		|| info.coding_independent_code_points.is_some()
		|| info.mastering_display_color_volume.is_some()
		|| info.content_light_level.is_some()
	{
		return Err(Error::BackgroundRetinaPngFormat);
	}

	match info.icc_profile.as_deref() {
		Some(profile) if info.srgb.is_none() && icc::valid_for_model(profile, gray) => {
			Ok(Some(profile))
		}
		// ImageIO treats untagged and sRGB RGB TIFF alike. Other color directives
		// require explicit conversion rather than silently changing the image.
		None if !has_icc_chunk(png)
			&& (info.srgb.is_some()
				|| (info.gama_chunk.is_none() && info.chrm_chunk.is_none())) =>
		{
			Ok(None)
		}
		_ => Err(Error::BackgroundRetinaPngFormat),
	}
}

// png skips unparseable ancillary ICC payloads without an error. Detect the
// presence of the chunk so the conversion cannot accidentally discard it.
fn has_icc_chunk(bytes: &[u8]) -> bool {
	let Some(mut chunks) = bytes.get(PNG_SIGNATURE_BYTES..) else {
		return false;
	};
	while let Some((header, body)) = chunks.split_first_chunk::<PNG_CHUNK_HEADER_BYTES>() {
		let (size, tag) = header.split_at(size_of::<u32>());
		if tag == png::chunk::iCCP.0 {
			return true;
		}
		if tag == png::chunk::IEND.0 {
			break;
		}
		let next = size
			.try_into()
			.ok()
			.map(u32::from_be_bytes)
			.and_then(|size| usize::try_from(size).ok())
			.and_then(|size| size.checked_add(PNG_CHUNK_CRC_BYTES))
			.and_then(|offset| body.get(offset..));
		let Some(next) = next else {
			break;
		};
		chunks = next;
	}
	false
}

fn decode_error(error: png::DecodingError) -> Error {
	match error {
		png::DecodingError::LimitsExceeded => Error::BackgroundMemory,
		other => Error::BackgroundPng(other),
	}
}
