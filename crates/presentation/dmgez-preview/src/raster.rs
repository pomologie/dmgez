//! Bounded CPU rendering of icon layouts to PNG.

#[cfg(feature = "actual")]
mod actual;
mod background;
mod canvas;
mod label;

use std::{collections::TryReserveError, io::Write, ops::Range, path::Path};

use dmgez_config::{Arrangement, BackgroundColorError, LabelPosition, Project, VirtualPath};
use thiserror::Error;

use crate::{
	CanvasPoint, DeclaredTree, DeclaredTreeError, FolderRect, GeometryError, ItemVisibility,
	Viewport,
};

#[cfg(feature = "actual")]
pub use actual::{ActualItem, ActualPreview};
use canvas::{Canvas, Dimensions};
use label::Labels;

const MAX_FOLDER_ENTRIES: usize = 512;
const PIXELS_PER_POINT: f64 = 1.0;
const PNG_SAMPLE_DEPTH: png::BitDepth = png::BitDepth::Eight;
const PNG_COLOR_TYPE: png::ColorType = png::ColorType::Rgb;
const PNG_DESCRIPTION_KEY: &str = "Description";
const PNG_DESCRIPTION: &str = "dmgez project design preview";

/// A design cannot be rendered or written within the qualified PNG profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Tree(#[from] DeclaredTreeError),
	#[error(transparent)]
	Geometry(#[from] GeometryError),
	#[error("the requested folder is absent from the declared project tree: {0:?}")]
	FolderNotFound(VirtualPath),
	#[error("PNG preview requires a manually arranged icon view")]
	Arrangement,
	#[error("custom icon artwork is outside the design PNG profile at {0:?}")]
	CustomIcon(VirtualPath),
	#[error("the folder exceeds the design PNG limit of {MAX_FOLDER_ENTRIES} entries")]
	EntryBudget,
	#[error("PNG dimensions exceed the raster limits")]
	Dimensions,
	#[error("PNG exceeds the raster work budget")]
	PaintBudget,
	#[error("the requested Finder background is outside the design PNG profile")]
	BackgroundProfile,
	#[error(transparent)]
	BackgroundColor(#[from] BackgroundColorError),
	#[error("PNG background source could not be opened")]
	BackgroundOpen(#[source] std::io::Error),
	#[error("PNG background source could not be read consistently")]
	BackgroundRead(#[from] dmgez_core::source::SourceError),
	#[error("PNG background source exceeds the input byte budget")]
	BackgroundBytes,
	#[error("PNG background could not be decoded")]
	BackgroundDecode(#[from] png::DecodingError),
	#[error("could not allocate bounded raster storage")]
	Allocation(#[from] TryReserveError),
	#[error("could not encode or write the PNG")]
	Png(#[from] png::EncodingError),
	#[cfg(feature = "actual")]
	#[error(transparent)]
	Actual(#[from] crate::actual::Error),
}

/// A prepared project design in one-pixel-per-point RGB.
#[derive(Debug)]
pub struct DesignPreview {
	canvas: Canvas,
}

impl DesignPreview {
	/// Renders one folder from the typed declaration tree and Finder settings.
	///
	/// Project-relative paths resolve against `project_directory`. Only the
	/// primary image of a PNG background can be read. Copy sources and custom
	/// icon contents are outside this renderer.
	///
	/// # Errors
	/// Returns an error for unsupported art, arrangement, or view, missing
	/// declared folders or required assets, invalid geometry, or exceeded
	/// raster budgets.
	pub fn from_project(
		project: &Project,
		project_directory: &Path,
		folder: &VirtualPath,
		visibility: ItemVisibility,
	) -> Result<Self, Error> {
		let tree = DeclaredTree::from_project(project)?;
		if !tree.contains_folder(folder) {
			return Err(Error::FolderNotFound(folder.clone()));
		}
		let children = tree.children(folder);
		if children.len() > MAX_FOLDER_ENTRIES {
			return Err(Error::EntryBudget);
		}
		let layout = tree.layout(project, folder)?;
		let style = layout.style();
		if style.arrangement != Arrangement::None {
			return Err(Error::Arrangement);
		}
		let visible = |item: &&crate::PlacedItem<'_>| {
			!item.hidden || visibility == ItemVisibility::IncludeHidden
		};
		let viewport = style.viewport(CanvasPoint::new(0.0, 0.0)?, PIXELS_PER_POINT)?;
		let dimensions = Dimensions::from_size(style.window_size)?;
		let mut canvas = background::render(dimensions, style.background, project_directory)?;
		let custom_icons = project
			.contents
			.iter()
			.filter_map(|content| content.custom_icon.as_ref().map(|_| &content.destination))
			.collect::<std::collections::BTreeSet<_>>();
		let mut labels = Labels::new(style.text_size);
		for item in layout.items().iter().filter(visible) {
			if custom_icons.contains(item.destination) {
				return Err(Error::CustomIcon(item.destination.clone()));
			}
			let directory = tree.contains_folder(item.destination);
			paint_item(
				&mut canvas,
				&mut labels,
				viewport,
				item.icon_bounds,
				item.display_name(),
				directory,
				style.label_position,
			)?;
		}
		Ok(Self { canvas })
	}

	/// Returns the dimensions of the PNG in pixels.
	#[must_use]
	pub const fn size(&self) -> (u32, u32) {
		self.canvas.dimensions().size()
	}

	/// Writes the prepared RGB raster to a PNG stream.
	///
	/// # Errors
	/// Returns an encoding or write error if the sink cannot receive the PNG.
	pub fn write_png(&self, output: impl Write) -> Result<(), Error> {
		write_png(&self.canvas, output, PNG_DESCRIPTION)
	}
}

fn paint_item(
	canvas: &mut Canvas,
	labels: &mut Labels,
	viewport: Viewport,
	bounds: FolderRect,
	name: &str,
	directory: bool,
	label_position: LabelPosition,
) -> Result<[Range<usize>; 2], Error> {
	let start = viewport.to_canvas(bounds.min())?;
	let end = viewport.to_canvas(bounds.max())?;
	let pixels = canvas.paint_icon(start, end, directory)?;
	labels.paint(canvas, name, start, end, label_position)?;
	Ok(pixels)
}

fn write_png(canvas: &Canvas, output: impl Write, description: &str) -> Result<(), Error> {
	let (width, height) = canvas.dimensions().size();
	let mut encoder = png::Encoder::new(output, width, height);
	encoder.set_color(PNG_COLOR_TYPE);
	encoder.set_depth(PNG_SAMPLE_DEPTH);
	encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
	encoder.add_text_chunk(PNG_DESCRIPTION_KEY.to_owned(), description.to_owned())?;
	let mut writer = encoder.write_header()?;
	writer.write_image_data(canvas.bytes())?;
	writer.finish()?;
	Ok(())
}
