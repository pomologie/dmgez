use std::{fs::File, io, io::Cursor, path::Path};

use dmgez_config::{Background, parse_solid_background_color};
use dmgez_core::source::CapturedFile;
use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

use super::{
	Error,
	canvas::{Canvas, Dimensions, Rgb},
};

const DEFAULT_BACKGROUND: Rgb = [245, 249, 248];
const MAX_ASSET_BYTES: usize = 16 << 20;
const MAX_DECODE_BYTES: usize = 16 << 20;
const FULL_OPACITY: u8 = u8::MAX;

pub(super) fn render(
	dimensions: Dimensions,
	background: Option<&Background>,
	project_directory: &Path,
) -> Result<Canvas, Error> {
	match background {
		None | Some(Background::None { .. }) => Canvas::new(dimensions, DEFAULT_BACKGROUND),
		Some(Background::Color { value }) => {
			let (red, green, blue) = parse_solid_background_color(value.as_str())?;
			let color =
				[red, green, blue].map(|channel| (channel * f64::from(u8::MAX)).round() as u8);
			Canvas::new(dimensions, color)
		}
		Some(Background::Image { source, .. }) => {
			let path = source.resolve(project_directory);
			let mut canvas = Canvas::new(dimensions, DEFAULT_BACKGROUND)?;
			paint_png(&mut canvas, &path)?;
			Ok(canvas)
		}
		_ => Err(Error::BackgroundProfile),
	}
}

fn paint_png(canvas: &mut Canvas, path: &Path) -> Result<(), Error> {
	if !path.metadata().map_err(Error::BackgroundOpen)?.is_file() {
		return Err(Error::BackgroundOpen(io::Error::new(
			io::ErrorKind::InvalidInput,
			"design background must be a regular file",
		)));
	}
	let source = CapturedFile::new(File::open(path).map_err(Error::BackgroundOpen)?)?;
	let input_bytes = usize::try_from(source.len())
		.ok()
		.filter(|bytes| *bytes <= MAX_ASSET_BYTES)
		.ok_or(Error::BackgroundBytes)?;
	let mut input = zeroed(input_bytes)?;
	source.read_exact_at(0, &mut input)?;
	let mut decoder = Decoder::new_with_limits(
		Cursor::new(input),
		Limits {
			bytes: MAX_DECODE_BYTES,
		},
	);
	decoder.set_ignore_text_chunk(true);
	decoder.set_transformations(Transformations::EXPAND);
	let mut reader = decoder.read_info()?;
	let info = reader.info();
	if info.animation_control().is_some()
		|| info.icc_profile.is_some()
		|| info.coding_independent_code_points.is_some()
		|| info.mastering_display_color_volume.is_some()
		|| info.content_light_level.is_some()
		|| (info.srgb.is_none() && (info.gama_chunk.is_some() || info.chrm_chunk.is_some()))
		|| info.bit_depth != BitDepth::Eight
	{
		return Err(Error::BackgroundProfile);
	}
	let size = Dimensions::new(info.width, info.height)?;
	let channels = match reader.output_color_type() {
		(ColorType::Rgb, BitDepth::Eight) => 3,
		(ColorType::Rgba, BitDepth::Eight) => 4,
		_ => return Err(Error::BackgroundProfile),
	};
	let output_bytes = reader.output_buffer_size().ok_or(Error::Dimensions)?;
	if output_bytes > MAX_DECODE_BYTES {
		return Err(Error::Dimensions);
	}
	let mut output = zeroed(output_bytes)?;
	let frame = reader.next_frame(&mut output)?;
	reader.finish()?;
	let (width, height) = size.size();
	if (frame.width, frame.height) != (width, height) {
		return Err(Error::BackgroundProfile);
	}
	canvas.composite([0.0, 0.0], [width, height], |x, y| {
		let offset = y * frame.line_size + x * channels;
		let color = [output[offset], output[offset + 1], output[offset + 2]];
		let alpha = if channels == 4 {
			output[offset + 3]
		} else {
			FULL_OPACITY
		};
		(color, alpha)
	})
}

fn zeroed(bytes: usize) -> Result<Vec<u8>, Error> {
	let mut result = Vec::new();
	result.try_reserve_exact(bytes)?;
	result.resize(bytes, 0);
	Ok(result)
}
