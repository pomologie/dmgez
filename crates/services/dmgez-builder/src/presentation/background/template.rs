use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype::RGB8};

use crate::{preflight::ExecutionProfile, presentation::check_cancelled};

use super::{
	Error, RETINA_SCALE,
	output::{BASE_DPI, CappedOutput, RETINA_DPI, tiff_encode_error, write_tiff_resolution},
};

#[cfg(test)]
mod tests;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 280;
const STANDARD_SCALE: u32 = 1;
const BACKGROUND_RGB: [u8; 3] = [246, 248, 251];
const ARROW_RGB: [u8; 3] = [30, 99, 174];

// Original arrow artwork centered at the default window's midpoint.
const SHAFT_LEFT: u32 = 236;
const HEAD_BASE: u32 = 335;
const TIP_RIGHT: u32 = WIDTH - SHAFT_LEFT;
const CENTER_Y: u32 = HEIGHT / 2;
const SHAFT_HALF_HEIGHT: u32 = 9;
const HEAD_HALF_HEIGHT: u32 = 33;
const SUBPIXEL_SCALE: u32 = 4;
const SAMPLE_OFFSETS: [u32; 2] = [1, 3];
const SAMPLES_PER_PIXEL: u32 = (SAMPLE_OFFSETS.len() * SAMPLE_OFFSETS.len()) as u32;

// A Retina strip uses at most 60 KiB of RGB samples. The fixed codec working
// allowance covers the pinned per-strip zlib encoder and the strip tables.
const ROWS_PER_STRIP: u32 = 16;
const CODEC_WORKSPACE_BYTES: usize = 1 << 20;

pub(super) fn render(
	profile: &ExecutionProfile,
	max_output_bytes: usize,
) -> Result<Vec<u8>, Error> {
	check_cancelled(profile)?;
	let mut output = CappedOutput::new(max_output_bytes);
	let mut encoder = TiffEncoder::new(&mut output)
		.map_err(tiff_encode_error)?
		.with_compression(Compression::Deflate(DeflateLevel::Balanced));
	for (scale, dpi) in [(STANDARD_SCALE, BASE_DPI), (RETINA_SCALE, RETINA_DPI)] {
		write_representation(&mut encoder, profile, scale, dpi)?;
	}
	check_cancelled(profile)?;
	Ok(output.into_bytes())
}

fn write_representation(
	encoder: &mut TiffEncoder<&mut CappedOutput>,
	profile: &ExecutionProfile,
	scale: u32,
	dpi: u32,
) -> Result<(), Error> {
	let width = WIDTH * scale;
	let height = HEIGHT * scale;
	let row_pixels = width as usize;
	let raster_pixels = row_pixels * height as usize;
	let allowed_pixels = profile
		.write_options()
		.max_buffer_bytes
		.checked_sub(CODEC_WORKSPACE_BYTES)
		.ok_or(Error::BackgroundMemory)?
		/ BACKGROUND_RGB.len();
	if raster_pixels > allowed_pixels {
		return Err(Error::BackgroundMemory);
	}
	// The pinned TIFF encoder activates Deflate in write_data. Keep one
	// fixed-size representation at a time, then release it before the next.
	let mut raster = Vec::new();
	raster.try_reserve_exact(raster_pixels)?;
	if raster.capacity() > allowed_pixels {
		return Err(Error::BackgroundMemory);
	}
	raster.resize(raster_pixels, BACKGROUND_RGB);
	let top = (CENTER_Y - HEAD_HALF_HEIGHT) * scale;
	let bottom = (CENTER_Y + HEAD_HALF_HEIGHT) * scale;
	let left = SHAFT_LEFT * scale;
	let right = TIP_RIGHT * scale;
	for y in top..bottom {
		check_cancelled(profile)?;
		let row_start = y as usize * row_pixels;
		let arrow_row = &mut raster[row_start + left as usize..row_start + right as usize];
		for (x, pixel) in (left..right).zip(arrow_row) {
			*pixel = pixel_color(x, y, scale);
		}
	}
	check_cancelled(profile)?;
	let mut page = encoder
		.new_image::<RGB8>(width, height)
		.map_err(tiff_encode_error)?;
	page.rows_per_strip(ROWS_PER_STRIP)
		.map_err(tiff_encode_error)?;
	write_tiff_resolution(page.encoder(), dpi).map_err(tiff_encode_error)?;
	page.write_data(raster.as_flattened())
		.map_err(tiff_encode_error)
}

fn pixel_color(x: u32, y: u32, scale: u32) -> [u8; 3] {
	let mut covered = 0;
	for dy in SAMPLE_OFFSETS {
		for dx in SAMPLE_OFFSETS {
			covered += u32::from(arrow_covers(
				x * SUBPIXEL_SCALE + dx,
				y * SUBPIXEL_SCALE + dy,
				scale * SUBPIXEL_SCALE,
			));
		}
	}
	std::array::from_fn(|channel| {
		let base = u32::from(BACKGROUND_RGB[channel]) * (SAMPLES_PER_PIXEL - covered);
		let arrow = u32::from(ARROW_RGB[channel]) * covered;
		((base + arrow + SAMPLES_PER_PIXEL / 2) / SAMPLES_PER_PIXEL) as u8
	})
}

fn arrow_covers(x: u32, y: u32, scale: u32) -> bool {
	let distance_from_center = y.abs_diff(CENTER_Y * scale);
	let head_base = HEAD_BASE * scale;
	let tip = TIP_RIGHT * scale;
	if x < head_base {
		distance_from_center < SHAFT_HALF_HEIGHT * scale
	} else {
		distance_from_center * (TIP_RIGHT - HEAD_BASE) <= (tip - x) * HEAD_HALF_HEIGHT
	}
}
