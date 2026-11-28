use std::{io::Cursor, num::NonZeroU64};

use image::{ImageDecoder, ImageFormat, codecs::bmp::BmpDecoder};
use zune_jpeg::{
	JpegDecoder,
	zune_core::{bytestream::ZCursor, options::DecoderOptions},
};

use crate::preflight::ExecutionProfile;

use super::{Error, MAX_RASTER_BYTES, PNG_EXTENSION, TIFF_EXTENSION, prebuilt, validate_png};

const BMP_EXTENSION: &str = ".bmp";
const GIF_EXTENSION: &str = ".gif";
const JPEG_EXTENSION: &str = ".jpg";
const WEBP_EXTENSION: &str = ".webp";
const MAX_STATIC_DIMENSION: u32 = 4096;
const MAX_STATIC_PIXELS: u64 = 4 << 20;
// Reserve space separately for decoder work (including progressive JPEG coefficients).
const STATIC_WORK_BYTES_PER_PIXEL: usize = 16;
const STATIC_DECODER_OVERHEAD_BYTES: usize = 512 << 10;

pub(super) fn validate(
	bytes: &[u8],
	available: usize,
	profile: &ExecutionProfile,
) -> Result<&'static str, Error> {
	if prebuilt::is_tiff(bytes) {
		prebuilt::validate(bytes, available, profile)?;
		return Ok(TIFF_EXTENSION);
	}
	let format = image::guess_format(bytes).map_err(|_| Error::BackgroundImageFormat)?;
	match format {
		ImageFormat::Png => validate_png(bytes, available).map(|()| PNG_EXTENSION),
		ImageFormat::Bmp => validate_bmp(bytes, available).map(|()| BMP_EXTENSION),
		ImageFormat::Gif => validate_gif(bytes, available).map(|()| GIF_EXTENSION),
		ImageFormat::Jpeg => validate_jpeg(bytes, available).map(|()| JPEG_EXTENSION),
		ImageFormat::WebP => validate_webp(bytes, available).map(|()| WEBP_EXTENSION),
		_ => Err(Error::BackgroundImageFormat),
	}
}

fn budget(width: u32, height: u32, available: usize) -> Result<usize, Error> {
	if !(1..=MAX_STATIC_DIMENSION).contains(&width) || !(1..=MAX_STATIC_DIMENSION).contains(&height)
	{
		return Err(Error::BackgroundMemory);
	}
	let pixels = u64::from(width) * u64::from(height);
	if pixels > MAX_STATIC_PIXELS {
		return Err(Error::BackgroundMemory);
	}
	let working = (pixels as usize) * STATIC_WORK_BYTES_PER_PIXEL + STATIC_DECODER_OVERHEAD_BYTES;
	available
		.checked_sub(working)
		.ok_or(Error::BackgroundMemory)
}

fn raster(size: Option<usize>, available: usize) -> Result<Vec<u8>, Error> {
	let size = size
		.filter(|size| (1..=available.min(MAX_RASTER_BYTES)).contains(size))
		.ok_or(Error::BackgroundMemory)?;
	let mut buffer = Vec::new();
	buffer.try_reserve_exact(size)?;
	if buffer.capacity() > available {
		return Err(Error::BackgroundMemory);
	}
	buffer.resize(size, 0);
	Ok(buffer)
}

fn validate_bmp(bytes: &[u8], available: usize) -> Result<(), Error> {
	let decoder = BmpDecoder::new(Cursor::new(bytes)).map_err(Error::BackgroundBmp)?;
	let (width, height) = decoder.dimensions();
	let available = budget(width, height, available)?;
	let mut output = raster(usize::try_from(decoder.total_bytes()).ok(), available)?;
	decoder
		.read_image(&mut output)
		.map_err(Error::BackgroundBmp)
}

fn validate_gif(bytes: &[u8], available: usize) -> Result<(), Error> {
	let mut options = gif::DecodeOptions::new();
	options.check_frame_consistency(true);
	options.check_lzw_end_code(true);
	let limit =
		NonZeroU64::new(available.min(MAX_RASTER_BYTES) as u64).ok_or(Error::BackgroundMemory)?;
	options.set_memory_limit(gif::MemoryLimit::Bytes(limit));
	let mut decoder = options
		.read_info(Cursor::new(bytes))
		.map_err(Error::BackgroundGif)?;
	let available = budget(
		u32::from(decoder.width()),
		u32::from(decoder.height()),
		available,
	)?;
	if decoder
		.next_frame_info()
		.map_err(Error::BackgroundGif)?
		.is_none()
	{
		return Err(Error::BackgroundImageFormat);
	}
	let mut output = raster(Some(decoder.buffer_size()), available)?;
	decoder
		.read_into_buffer(&mut output)
		.map_err(Error::BackgroundGif)?;
	if decoder
		.next_frame_info()
		.map_err(Error::BackgroundGif)?
		.is_some()
	{
		return Err(Error::BackgroundAnimated);
	}
	Ok(())
}

fn validate_jpeg(bytes: &[u8], available: usize) -> Result<(), Error> {
	let options = DecoderOptions::default()
		.set_max_width(MAX_STATIC_DIMENSION as usize)
		.set_max_height(MAX_STATIC_DIMENSION as usize);
	let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
	decoder.decode_headers().map_err(Error::BackgroundJpeg)?;
	let (width, height) = decoder.dimensions().ok_or(Error::BackgroundImageFormat)?;
	let available = budget(width as u32, height as u32, available)?;
	let mut output = raster(decoder.output_buffer_size(), available)?;
	decoder
		.decode_into(&mut output)
		.map_err(Error::BackgroundJpeg)
}

fn validate_webp(bytes: &[u8], available: usize) -> Result<(), Error> {
	let mut decoder =
		image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(Error::BackgroundWebp)?;
	if decoder.is_animated() {
		return Err(Error::BackgroundAnimated);
	}
	let (width, height) = decoder.dimensions();
	let raster_budget = budget(width, height, available)?;
	decoder.set_memory_limit(available - raster_budget);
	let mut output = raster(decoder.output_buffer_size(), raster_budget)?;
	decoder
		.read_image(&mut output)
		.map_err(Error::BackgroundWebp)
}
