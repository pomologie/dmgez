use std::{fs::File, io, path::Path};

use crate::png_image::{self, BACKGROUND_INPUT_BYTES};
use dmgez_config::{Background, parse_solid_background_color};
use dmgez_core::source::CapturedFile;

use super::{
	Error,
	canvas::{Canvas, Dimensions, Rgb},
};

pub(super) const DEFAULT_BACKGROUND: Rgb = [245, 249, 248];

pub(super) fn render(
	dimensions: Dimensions,
	background: Option<&Background>,
	project_directory: &Path,
) -> Result<Canvas, Error> {
	match background {
		None | Some(Background::None { .. }) => Canvas::new(dimensions, DEFAULT_BACKGROUND),
		Some(Background::Color { value }) => {
			let (red, green, blue) = parse_solid_background_color(value.as_str())?;
			color_canvas(dimensions, [red, green, blue])
		}
		Some(Background::Image { source, .. }) => {
			let path = source.resolve(project_directory);
			let mut canvas = Canvas::new(dimensions, DEFAULT_BACKGROUND)?;
			paint_file(&mut canvas, &path)?;
			Ok(canvas)
		}
		_ => Err(Error::BackgroundProfile),
	}
}

pub(super) fn color_canvas(dimensions: Dimensions, channels: [f64; 3]) -> Result<Canvas, Error> {
	let color = channels.map(|channel| (channel * f64::from(u8::MAX)).round() as u8);
	Canvas::new(dimensions, color)
}

fn paint_file(canvas: &mut Canvas, path: &Path) -> Result<(), Error> {
	if !path.metadata().map_err(Error::BackgroundOpen)?.is_file() {
		return Err(Error::BackgroundOpen(io::Error::new(
			io::ErrorKind::InvalidInput,
			"design background must be a regular file",
		)));
	}
	let source = CapturedFile::new(File::open(path).map_err(Error::BackgroundOpen)?)?;
	let input_bytes = usize::try_from(source.len())
		.ok()
		.filter(|bytes| *bytes <= BACKGROUND_INPUT_BYTES)
		.ok_or(Error::BackgroundBytes)?;
	let mut input = zeroed(input_bytes)?;
	source.read_exact_at(0, &mut input)?;
	paint_png(canvas, &input)
}

pub(super) fn paint_png(canvas: &mut Canvas, input: &[u8]) -> Result<(), Error> {
	let image = png_image::decode_background(input).map_err(|error| match error {
		png_image::Error::InputBytes => Error::BackgroundBytes,
		png_image::Error::Dimensions => Error::Dimensions,
		png_image::Error::DecoderBudget => {
			Error::BackgroundDecode(png::DecodingError::LimitsExceeded)
		}
		png_image::Error::Allocation(error) => Error::Allocation(error),
		png_image::Error::Decode(error) => Error::BackgroundDecode(error),
		_ => Error::BackgroundProfile,
	})?;
	canvas.composite([0.0, 0.0], image.size().map(|side| side as u32), |x, y| {
		let [red, green, blue, alpha] = image.rgba_at(x, y);
		([red, green, blue], alpha)
	})
}

fn zeroed(bytes: usize) -> Result<Vec<u8>, Error> {
	let mut result = Vec::new();
	result.try_reserve_exact(bytes)?;
	result.resize(bytes, 0);
	Ok(result)
}
