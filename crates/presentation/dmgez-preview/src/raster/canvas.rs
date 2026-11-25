use std::ops::Range;

use crate::{CanvasPoint, Size};

use super::Error;

pub(super) type Rgb = [u8; 3];
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: usize = 4 << 20;
const MAX_PAINT_PIXELS: usize = 64 << 20;
const FULL_OPACITY: u8 = u8::MAX;
const BORDER_PIXELS: f64 = 1.0;
const ICON_EDGE: Rgb = [103, 132, 129];
const ICON_SURFACE: Rgb = [215, 234, 229];
const ICON_MARK: Rgb = [34, 56, 60];
const MARK_INSET_FRACTION: f64 = 0.29;
const FOLDER_TAB_FRACTION: f64 = 0.42;
const FOLDER_TOP_FRACTION: f64 = 0.22;
const FOLDER_BODY_TOP_FRACTION: f64 = 0.34;

#[derive(Clone, Copy, Debug)]
pub(super) struct Dimensions {
	width: u32,
	height: u32,
	pixels: usize,
}

impl Dimensions {
	pub(super) fn from_size(size: Size) -> Result<Self, Error> {
		let side = |points: f64| {
			let rounded = points.ceil();
			if (1.0..=f64::from(MAX_SIDE)).contains(&rounded) {
				Ok(rounded as u32)
			} else {
				Err(Error::Dimensions)
			}
		};
		Self::new(side(size.width())?, side(size.height())?)
	}

	pub(super) fn new(width: u32, height: u32) -> Result<Self, Error> {
		let pixels = usize::try_from(u64::from(width) * u64::from(height))
			.ok()
			.filter(|pixels| (1..=MAX_PIXELS).contains(pixels))
			.ok_or(Error::Dimensions)?;
		if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
			return Err(Error::Dimensions);
		}
		Ok(Self {
			width,
			height,
			pixels,
		})
	}

	pub(super) const fn size(self) -> (u32, u32) {
		(self.width, self.height)
	}
}

pub(super) struct Canvas {
	dimensions: Dimensions,
	pixels: Vec<Rgb>,
	remaining_paint_pixels: usize,
}

impl std::fmt::Debug for Canvas {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Canvas")
			.field("dimensions", &self.dimensions)
			.finish_non_exhaustive()
	}
}

impl Canvas {
	pub(super) fn new(dimensions: Dimensions, color: Rgb) -> Result<Self, Error> {
		let mut pixels = Vec::new();
		pixels.try_reserve_exact(dimensions.pixels)?;
		pixels.resize(dimensions.pixels, color);
		Ok(Self {
			dimensions,
			pixels,
			remaining_paint_pixels: MAX_PAINT_PIXELS,
		})
	}

	pub(super) const fn dimensions(&self) -> Dimensions {
		self.dimensions
	}

	pub(super) fn bytes(&self) -> &[u8] {
		self.pixels.as_flattened()
	}

	pub(super) fn paint_icon(
		&mut self,
		min: CanvasPoint,
		max: CanvasPoint,
		directory: bool,
	) -> Result<[Range<usize>; 2], Error> {
		let mut outer = Rect {
			left: min.x(),
			top: min.y(),
			right: max.x(),
			bottom: max.y(),
		};
		let bounds = self.clip(outer);
		self.outline(outer, ICON_EDGE, ICON_SURFACE)?;
		let inset = (outer.right - outer.left) * MARK_INSET_FRACTION;
		outer = outer.inset(inset);
		if directory {
			let height = max.y() - min.y();
			let tab = Rect {
				top: min.y() + height * FOLDER_TOP_FRACTION,
				right: min.x() + (max.x() - min.x()) * FOLDER_TAB_FRACTION,
				..outer
			};
			self.paint(tab, ICON_MARK)?;
			self.paint(
				Rect {
					top: min.y() + height * FOLDER_BODY_TOP_FRACTION,
					..outer
				},
				ICON_MARK,
			)?;
		} else {
			self.outline(outer, ICON_MARK, ICON_SURFACE)?;
		}
		Ok(bounds)
	}

	pub(super) fn paint(&mut self, rect: Rect, color: Rgb) -> Result<(), Error> {
		let [columns, rows] = self.clip(rect);
		self.charge(&columns, &rows)?;
		let stride = self.dimensions.width as usize;
		for row in rows {
			let start = row * stride;
			self.pixels[start + columns.start..start + columns.end].fill(color);
		}
		Ok(())
	}

	pub(super) fn composite(
		&mut self,
		origin: [f64; 2],
		size: [u32; 2],
		mut sample: impl FnMut(usize, usize) -> (Rgb, u8),
	) -> Result<(), Error> {
		let [left, top] = origin.map(f64::round);
		let [columns, rows] = self.clip(Rect {
			left,
			top,
			right: left + f64::from(size[0]),
			bottom: top + f64::from(size[1]),
		});
		self.charge(&columns, &rows)?;
		if columns.is_empty() || rows.is_empty() {
			return Ok(());
		}
		let stride = self.dimensions.width as usize;
		for y in rows {
			for x in columns.clone() {
				// A nonempty clip keeps this difference within the source extent.
				let (rgb, alpha) = sample((x as f64 - left) as usize, (y as f64 - top) as usize);
				let pixel = &mut self.pixels[y * stride + x];
				match alpha {
					0 => (),
					FULL_OPACITY => *pixel = rgb,
					_ => {
						let weight = u16::from(alpha);
						let max = u16::from(FULL_OPACITY);
						for (destination, source) in pixel.iter_mut().zip(rgb) {
							let mixed = u16::from(source) * weight
								+ u16::from(*destination) * (max - weight);
							*destination = ((mixed + max / 2) / max) as u8;
						}
					}
				}
			}
		}
		Ok(())
	}

	fn outline(&mut self, rect: Rect, edge: Rgb, surface: Rgb) -> Result<(), Error> {
		self.paint(rect, edge)?;
		self.paint(rect.inset(BORDER_PIXELS), surface)
	}

	fn charge(&mut self, columns: &Range<usize>, rows: &Range<usize>) -> Result<(), Error> {
		self.remaining_paint_pixels = self
			.remaining_paint_pixels
			.checked_sub(columns.len() * rows.len())
			.ok_or(Error::PaintBudget)?;
		Ok(())
	}

	fn clip(&self, rect: Rect) -> [Range<usize>; 2] {
		fn axis(low: f64, high: f64, limit: u32) -> Range<usize> {
			let clipped = |value: f64| value.clamp(0.0, f64::from(limit)) as usize;
			let start = clipped(low.floor());
			start..clipped(high.ceil()).max(start)
		}
		if rect.left >= rect.right || rect.top >= rect.bottom {
			return [0..0, 0..0];
		}
		[
			axis(rect.left, rect.right, self.dimensions.width),
			axis(rect.top, rect.bottom, self.dimensions.height),
		]
	}
}

#[derive(Clone, Copy)]
pub(super) struct Rect {
	pub(super) left: f64,
	pub(super) top: f64,
	pub(super) right: f64,
	pub(super) bottom: f64,
}

impl Rect {
	fn inset(self, margin: f64) -> Self {
		Self {
			left: self.left + margin,
			top: self.top + margin,
			right: self.right - margin,
			bottom: self.bottom - margin,
		}
	}
}
