use std::{io::Write, ops::Range};

use crate::{
	CanvasPoint, ItemVisibility,
	actual::{
		Background, BackgroundAsset, BackgroundStatus, Coverage, Entry, EntryKind, Scene, Store,
	},
	png_image::SIGNATURE,
};

use super::{
	Error, PIXELS_PER_POINT, background,
	canvas::{Canvas, Dimensions},
	label::Labels,
	paint_item, write_png,
};

const PNG_DESCRIPTION: &str = "dmgez actual-image preview";

/// One actual-image preview viewport in one-pixel-per-point RGB.
#[derive(Debug)]
pub struct ActualPreview {
	canvas: Canvas,
	coverage: Coverage,
	items: Vec<ActualItem>,
}

/// The clipped icon region and stored identity of a visible image entry.
#[derive(Debug)]
#[non_exhaustive]
pub struct ActualItem {
	/// Exact stored UTF-16 name, including any unpaired units.
	pub name: Vec<u16>,
	/// Entry kind observed in the catalog.
	pub kind: EntryKind,
	/// Horizontal pixel range in the raster.
	pub columns: Range<usize>,
	/// Vertical pixel range in the raster.
	pub rows: Range<usize>,
}

impl ActualPreview {
	/// Renders one observed directory without opening any host or image paths.
	///
	/// `entries`, `store`, and `asset` must describe one immutable image
	/// observation. The supplied asset must be checked against the stored
	/// alias before passing [`BackgroundAsset::Encoded`].
	///
	/// # Errors
	/// Returns an error when input names, dimensions or rendering work exceed
	/// the admitted limits, or when the raster cannot be allocated.
	pub fn from_image(
		entries: &[Entry],
		store: Store<'_>,
		asset: BackgroundAsset<'_>,
		visibility: ItemVisibility,
	) -> Result<Self, Error> {
		let scene = Scene::from_image(entries, store, asset, visibility)?;
		let style = scene.style;
		let viewport = style.viewport(CanvasPoint::new(0.0, 0.0)?, PIXELS_PER_POINT)?;
		let dimensions = Dimensions::from_size(style.window_size)?;
		let (mut canvas, background) = paint_background(dimensions, scene.background)?;
		let mut labels = Labels::new(style.text_size);
		let mut items = Vec::new();
		items.try_reserve_exact(scene.items.len())?;
		for item in scene.items {
			let [columns, rows] = paint_item(
				&mut canvas,
				&mut labels,
				viewport,
				item.bounds,
				item.label(),
				item.is_directory(),
				style.label_position,
			)?;
			if !columns.is_empty() && !rows.is_empty() {
				let mut name = Vec::new();
				name.try_reserve_exact(item.entry.name.len())?;
				name.extend_from_slice(&item.entry.name);
				items.push(ActualItem {
					name,
					kind: item.entry.kind,
					columns,
					rows,
				});
			}
		}
		let coverage = Coverage {
			background,
			..scene.coverage
		};
		Ok(Self {
			canvas,
			coverage,
			items,
		})
	}

	/// The raster's width and height in pixels.
	#[must_use]
	pub const fn size(&self) -> (u32, u32) {
		self.canvas.dimensions().size()
	}

	/// Packed RGB bytes in top-to-bottom row order.
	#[must_use]
	pub fn rgb(&self) -> &[u8] {
		self.canvas.bytes()
	}

	/// Observed and fallback metadata for this render.
	#[must_use]
	pub const fn coverage(&self) -> Coverage {
		self.coverage
	}

	/// Consumes the preview and returns the icon targets in paint order.
	#[must_use]
	pub fn into_items(self) -> Vec<ActualItem> {
		self.items
	}

	/// Writes an actual-image PNG to the supplied stream.
	///
	/// # Errors
	/// Returns an encoding or write error if the stream cannot receive the PNG.
	pub fn write_png(&self, output: impl Write) -> Result<(), Error> {
		write_png(&self.canvas, output, PNG_DESCRIPTION)
	}
}

fn paint_background(
	dimensions: Dimensions,
	plan: Background<'_>,
) -> Result<(Canvas, BackgroundStatus), Error> {
	match plan {
		Background::Color(channels) => Ok((
			background::color_canvas(dimensions, channels)?,
			BackgroundStatus::ObservedColor,
		)),
		Background::Default(status) => Ok((
			Canvas::new(dimensions, background::DEFAULT_BACKGROUND)?,
			status,
		)),
		Background::Image(bytes) => {
			let mut canvas = Canvas::new(dimensions, background::DEFAULT_BACKGROUND)?;
			let status = paint_image(&mut canvas, bytes)?;
			Ok((canvas, status))
		}
	}
}

fn paint_image(canvas: &mut Canvas, bytes: &[u8]) -> Result<BackgroundStatus, Error> {
	if !bytes.starts_with(&SIGNATURE) {
		return Ok(if SIGNATURE.starts_with(bytes) {
			BackgroundStatus::MalformedImage
		} else {
			BackgroundStatus::UnsupportedImage
		});
	}
	match background::paint_png(canvas, bytes) {
		Ok(()) => Ok(BackgroundStatus::ObservedPng),
		Err(
			Error::BackgroundProfile
			| Error::BackgroundBytes
			| Error::Dimensions
			| Error::BackgroundDecode(png::DecodingError::LimitsExceeded),
		) => Ok(BackgroundStatus::UnsupportedImage),
		Err(Error::BackgroundDecode(_)) => Ok(BackgroundStatus::MalformedImage),
		Err(error) => Err(error),
	}
}
