use std::ops::Range;

use super::{BadgeRaster, Error, OPAQUE, RGBA_CHANNELS};

type Pixel = [u8; RGBA_CHANNELS];

const CANVAS_UNITS: u32 = 1024;
const BADGE_CENTER: u32 = 736;
const BADGE_MAX_SIDE: u32 = 448;
const SUBPIXELS: u32 = 4;
const SUBPIXEL_CENTERS: [u32; 2] = [1, 3];
const SUBSAMPLES: u64 = (SUBPIXEL_CENTERS.len() * SUBPIXEL_CENTERS.len()) as u64;
const MAX_CHANNEL: u64 = OPAQUE as u64;

// Original drive glyph drawn from layered rounded rectangles on a unit grid.
const CASE: RoundedRect = RoundedRect::new(110, 172, 914, 852, 116);
const FRONT: RoundedRect = RoundedRect::new(125, 607, 899, 835, 80);
const LID: RoundedRect = RoundedRect::new(125, 187, 899, 664, 98);
const RECESS: RoundedRect = RoundedRect::new(208, 265, 816, 544, 77);
const INLAY: RoundedRect = RoundedRect::new(228, 285, 796, 524, 58);
const REFLECTION: RoundedRect = RoundedRect::new(266, 315, 690, 343, 14);
const LED: RoundedRect = RoundedRect::new(231, 711, 299, 775, 30);

const TRANSPARENT: Pixel = [0; RGBA_CHANNELS];
const CASE_COLOR: Pixel = [15, 32, 51, OPAQUE];
const FRONT_COLOR: Pixel = [24, 55, 83, OPAQUE];
const LID_COLOR: Pixel = [52, 94, 127, OPAQUE];
const RECESS_COLOR: Pixel = [37, 73, 104, OPAQUE];
const INLAY_COLOR: Pixel = [84, 136, 171, OPAQUE];
const REFLECTION_COLOR: Pixel = [148, 198, 223, OPAQUE];
const LED_COLOR: Pixel = [132, 224, 185, OPAQUE];

pub(super) fn render(badge: &BadgeRaster, side: u32) -> Result<Vec<u8>, Error> {
	let placement = BadgePlacement::new(badge, side);
	let mut pixels = Vec::new();
	pixels.try_reserve_exact(side as usize * side as usize * RGBA_CHANNELS)?;
	for y in 0..side {
		for x in 0..side {
			let base = volume_pixel(x, y, side);
			let pixel = if let Some(overlay) = placement.sample(badge, x, y) {
				source_over(overlay, base)
			} else {
				base
			};
			pixels.extend_from_slice(&pixel);
		}
	}
	Ok(pixels)
}

struct BadgePlacement {
	x: Range<u32>,
	y: Range<u32>,
}

impl BadgePlacement {
	fn new(badge: &BadgeRaster, side: u32) -> Self {
		let longest = badge.width.max(badge.height);
		let limit = (side * BADGE_MAX_SIDE + CANVAS_UNITS / 2) / CANVAS_UNITS;
		let fit = |dimension: u32| ((dimension * limit + longest / 2) / longest).max(1);
		let centered = |length| {
			let start = side * BADGE_CENTER / CANVAS_UNITS - length / 2;
			start..start + length
		};
		Self {
			x: centered(fit(badge.width)),
			y: centered(fit(badge.height)),
		}
	}

	fn sample(&self, badge: &BadgeRaster, x: u32, y: u32) -> Option<Pixel> {
		if !self.x.contains(&x) || !self.y.contains(&y) {
			return None;
		}
		let xs = SourceSpan::new(x - self.x.start, badge.width, self.x.end - self.x.start);
		let ys = SourceSpan::new(y - self.y.start, badge.height, self.y.end - self.y.start);
		let mut samples = Samples::default();
		for sy in ys.pixels() {
			for sx in xs.pixels() {
				let weight = u64::from(xs.coverage(sx)) * u64::from(ys.coverage(sy));
				samples.add(badge.pixel(sx, sy), weight);
			}
		}
		Some(samples.pixel(u64::from(badge.width) * u64::from(badge.height)))
	}
}

// In this coordinate system each source pixel is `scale` units wide.
// A target footprint intersects source pixels with exact integer overlap.
struct SourceSpan {
	start: u32,
	end: u32,
	scale: u32,
}

impl SourceSpan {
	fn new(target_pixel: u32, source_size: u32, target_size: u32) -> Self {
		let start = target_pixel * source_size;
		Self {
			start,
			end: start + source_size,
			scale: target_size,
		}
	}

	fn pixels(&self) -> Range<u32> {
		self.start / self.scale..self.end.div_ceil(self.scale)
	}

	fn coverage(&self, source_pixel: u32) -> u32 {
		let start = source_pixel * self.scale;
		self.end.min(start + self.scale) - self.start.max(start)
	}
}

#[derive(Default)]
struct Samples {
	color: [u64; 3],
	alpha: u64,
}

impl Samples {
	// Accumulate premultiplied channel samples so transparent source colors
	// cannot bleed into the edges while resizing or smoothing the glyph.
	fn add(&mut self, [red, green, blue, alpha]: Pixel, weight: u64) {
		let alpha = weight * u64::from(alpha);
		self.alpha += alpha;
		for (sum, channel) in self.color.iter_mut().zip([red, green, blue]) {
			*sum += alpha * u64::from(channel);
		}
	}

	fn pixel(&self, weight: u64) -> Pixel {
		let alpha = round_sample(self.alpha, weight);
		if alpha == 0 {
			return TRANSPARENT;
		}
		let [red, green, blue] = self.color.map(|sum| round_sample(sum, self.alpha));
		[red, green, blue, alpha]
	}
}

fn source_over(overlay: Pixel, base: Pixel) -> Pixel {
	let mut samples = Samples::default();
	samples.add(overlay, MAX_CHANNEL);
	samples.add(base, MAX_CHANNEL - u64::from(overlay[3]));
	samples.pixel(MAX_CHANNEL)
}

fn round_sample(numerator: u64, denominator: u64) -> u8 {
	// Each quotient is a weighted average or normalized coverage of u8 samples.
	((numerator + denominator / 2) / denominator) as u8
}

fn volume_pixel(x: u32, y: u32, side: u32) -> Pixel {
	let coordinate = |pixel, offset| (SUBPIXELS * pixel + offset) * CANVAS_UNITS / side;
	let mut samples = Samples::default();
	for offset_y in SUBPIXEL_CENTERS {
		for offset_x in SUBPIXEL_CENTERS {
			samples.add(
				volume_at(coordinate(x, offset_x), coordinate(y, offset_y)),
				1,
			);
		}
	}
	samples.pixel(SUBSAMPLES)
}

fn volume_at(x: u32, y: u32) -> Pixel {
	[
		(REFLECTION, REFLECTION_COLOR),
		(INLAY, INLAY_COLOR),
		(RECESS, RECESS_COLOR),
		(LID, LID_COLOR),
		(LED, LED_COLOR),
		(FRONT, FRONT_COLOR),
		(CASE, CASE_COLOR),
	]
	.into_iter()
	.find_map(|(region, color)| region.contains(x, y).then_some(color))
	.unwrap_or(TRANSPARENT)
}

struct RoundedRect {
	left: u32,
	top: u32,
	right: u32,
	bottom: u32,
	radius: u32,
}

impl RoundedRect {
	const fn new(left: u32, top: u32, right: u32, bottom: u32, radius: u32) -> Self {
		Self {
			left: left * SUBPIXELS,
			top: top * SUBPIXELS,
			right: right * SUBPIXELS,
			bottom: bottom * SUBPIXELS,
			radius: radius * SUBPIXELS,
		}
	}

	fn contains(&self, x: u32, y: u32) -> bool {
		if !(self.left..self.right).contains(&x) || !(self.top..self.bottom).contains(&y) {
			return false;
		}
		let dx = x.abs_diff(x.clamp(self.left + self.radius, self.right - self.radius));
		let dy = y.abs_diff(y.clamp(self.top + self.radius, self.bottom - self.radius));
		dx * dx + dy * dy <= self.radius * self.radius
	}
}
