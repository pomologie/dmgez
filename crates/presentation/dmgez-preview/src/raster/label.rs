use dmgez_config::LabelPosition;
use epaint::{
	Color32, FontId,
	text::{FontDefinitions, Fonts, LayoutJob, TextOptions, ViewportKey},
};

use crate::CanvasPoint;

use super::{
	Error,
	canvas::{Canvas, Rect, Rgb},
};

const MAX_LABEL_POINTS: f64 = 36.0;
const MAX_LABEL_PIXELS: f32 = 512.0;
const MAX_LABEL_SCALARS: usize = 128;
const MAX_FONT_TEXTURE_SIDE: usize = 1024;
const PIXELS_PER_POINT: f32 = 1.0;
const REPLACEMENT: char = '\u{fffd}';
const ELLIPSIS: char = '\u{2026}';
const LABEL_MARGIN: f64 = 5.0;
const LABEL_PAD_X: f64 = 4.0;
const LABEL_PAD_Y: f64 = 2.0;
const LABEL_SURFACE: Rgb = [245, 249, 248];
const LABEL_INK: Rgb = [34, 56, 60];

pub(super) struct Labels {
	fonts: Fonts,
	font_id: FontId,
}

impl Labels {
	pub(super) fn new(text_size: f64) -> Self {
		Self {
			fonts: Fonts::new(text_options(), FontDefinitions::default()),
			font_id: FontId::proportional(text_size.min(MAX_LABEL_POINTS) as f32),
		}
	}

	pub(super) fn paint(
		&mut self,
		canvas: &mut Canvas,
		name: &str,
		min: CanvasPoint,
		max: CanvasPoint,
		position: LabelPosition,
	) -> Result<(), Error> {
		self.fonts
			.begin_pass(text_options(), ViewportKey::default());
		let mut job =
			LayoutJob::simple_singleline(bounded_label(name), self.font_id.clone(), Color32::WHITE);
		job.wrap.max_width = (canvas.dimensions().size().0 as f32).min(MAX_LABEL_PIXELS);
		job.wrap.max_rows = 1;
		job.wrap.break_anywhere = true;
		let layout = self
			.fonts
			.with_pixels_per_point(PIXELS_PER_POINT)
			.layout_job(job);
		if layout.is_empty() {
			return Ok(());
		}
		let size = layout.size();
		let width = f64::from(size.x);
		let height = f64::from(size.y);
		let (left, top) = match position {
			LabelPosition::Right => (
				max.x() + LABEL_MARGIN,
				min.y().midpoint(max.y()) - height / 2.0,
			),
			_ => (
				min.x().midpoint(max.x()) - width / 2.0,
				max.y() + LABEL_MARGIN,
			),
		};
		canvas.paint(
			Rect {
				left: left - LABEL_PAD_X,
				top: top - LABEL_PAD_Y,
				right: left + width + LABEL_PAD_X,
				bottom: top + height + LABEL_PAD_Y,
			},
			LABEL_SURFACE,
		)?;
		let atlas = self.fonts.texture_atlas().image();
		for row in &layout.rows {
			for glyph in &row.glyphs {
				let uv = glyph.uv_rect;
				if uv.is_nothing() {
					continue;
				}
				let glyph_position = row.pos + glyph.pos.to_vec2() + uv.offset;
				let origin = [
					left + f64::from(glyph_position.x),
					top + f64::from(glyph_position.y),
				];
				let size = [
					u32::from(uv.max[0] - uv.min[0]),
					u32::from(uv.max[1] - uv.min[1]),
				];
				canvas.composite(origin, size, |x, y| {
					let index =
						(usize::from(uv.min[1]) + y) * atlas.size[0] + usize::from(uv.min[0]) + x;
					(LABEL_INK, atlas.pixels[index].a())
				})?;
			}
		}
		Ok(())
	}
}

fn text_options() -> TextOptions {
	TextOptions {
		max_texture_side: MAX_FONT_TEXTURE_SIDE,
		..TextOptions::default()
	}
}

fn bounded_label(name: &str) -> String {
	let mut scalars = name.chars();
	let mut label: String = scalars
		.by_ref()
		.take(MAX_LABEL_SCALARS)
		.map(|scalar| {
			if scalar.is_control() {
				REPLACEMENT
			} else {
				scalar
			}
		})
		.collect();
	if scalars.next().is_some() {
		label.push(ELLIPSIS);
	}
	label
}
