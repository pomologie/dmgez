use dmgez_config::{Arrangement, LabelPosition};
use dmgez_finder::inspection::{IconView, Window};

use crate::{FinderScreenPoint, FolderPoint, FolderStyle, GeometryError, Size};

const FALLBACK_WINDOW_WIDTH: f64 = 640.0;
const FALLBACK_WINDOW_HEIGHT: f64 = 480.0;
const FALLBACK_ICON_POINTS: f64 = 64.0;
const FALLBACK_TEXT_POINTS: f64 = 14.0;
const FALLBACK_GRID_SPACING: f64 = 110.0;
const FALLBACK_GRID_MARGIN: f64 = 16.0;
const MIN_DIMENSION: f64 = 1.0;
const MAX_POINT: f64 = u32::MAX as f64;

pub(super) fn fallback() -> Result<FolderStyle<'static>, GeometryError> {
	Ok(FolderStyle {
		window_origin: FinderScreenPoint::new(0.0, 0.0)?,
		window_size: Size::new(FALLBACK_WINDOW_WIDTH, FALLBACK_WINDOW_HEIGHT)?,
		scroll: FolderPoint::new(0.0, 0.0)?,
		grid_offset: FolderPoint::new(FALLBACK_GRID_MARGIN, FALLBACK_GRID_MARGIN)?,
		grid_spacing: FALLBACK_GRID_SPACING,
		icon_size: Size::new(FALLBACK_ICON_POINTS, FALLBACK_ICON_POINTS)?,
		text_size: FALLBACK_TEXT_POINTS,
		label_position: LabelPosition::Bottom,
		arrangement: Arrangement::None,
		background: None,
	})
}

pub(super) fn observed(
	mut style: FolderStyle<'static>,
	window: Option<&Window>,
	icons: Option<&IconView>,
) -> Result<(FolderStyle<'static>, bool), GeometryError> {
	let bounds = window.and_then(|window| window.bounds);
	if let Some(bounds) = bounds {
		style.window_size = Size::new(dimension(bounds.width)?, dimension(bounds.height)?)?;
		style.window_origin = FinderScreenPoint::new(bounds.x, bounds.y)?;
	}
	let Some(icons) = icons else {
		return Ok((style, false));
	};
	if let Some(scroll) = icons.scroll_position {
		style.scroll = point(scroll)?;
	}
	if let Some(offset) = icons.grid_offset {
		style.grid_offset = point(offset)?;
	}
	if let Some(spacing) = icons.grid_spacing {
		style.grid_spacing = dimension(spacing)?;
	}
	if let Some(size) = icons.icon_size {
		let size = dimension(size)?;
		style.icon_size = Size::new(size, size)?;
	}
	if let Some(size) = icons.text_size {
		style.text_size = dimension(size)?;
	}
	if let Some(bottom) = icons.label_on_bottom {
		style.label_position = if bottom {
			LabelPosition::Bottom
		} else {
			LabelPosition::Right
		};
	}
	let complete = bounds.is_some()
		&& icons.scroll_position.is_some()
		&& icons.grid_offset.is_some()
		&& icons.grid_spacing.is_some()
		&& icons.icon_size.is_some()
		&& icons.text_size.is_some()
		&& icons.label_on_bottom.is_some()
		&& icons.arrangement.is_some();
	Ok((style, complete))
}

fn dimension(points: f64) -> Result<f64, GeometryError> {
	if (MIN_DIMENSION..=MAX_POINT).contains(&points) {
		Ok(points)
	} else {
		Err(GeometryError::Size)
	}
}

fn point([x, y]: [f64; 2]) -> Result<FolderPoint, GeometryError> {
	if x.abs() > MAX_POINT || y.abs() > MAX_POINT {
		return Err(GeometryError::NonFinite);
	}
	FolderPoint::new(x, y)
}
