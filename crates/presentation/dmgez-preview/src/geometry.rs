use std::marker::PhantomData;

use thiserror::Error;

/// A coordinate transform or geometric extent is outside the finite domain.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum GeometryError {
	#[error("coordinate or computed result is outside the finite numeric domain")]
	NonFinite,
	#[error("size must be finite and strictly positive")]
	Size,
	#[error("scale must have a finite positive value and reciprocal")]
	Scale,
	#[error("grid spacing must be finite and strictly positive")]
	GridSpacing,
	#[error("rectangle edges must enclose a positive area at the given precision")]
	Rectangle,
}

/// Marker for folder-local points, with y increasing downwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FolderSpace {}

/// Marker for canvas coordinates, with y increasing downwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CanvasSpace {}

/// Marker for Finder window origins, with y increasing upwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FinderScreenSpace {}

/// A point in a named coordinate space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point<Space> {
	x: f64,
	y: f64,
	space: PhantomData<Space>,
}

/// A location inside a folder measured in logical points.
pub type FolderPoint = Point<FolderSpace>;
/// A location on the output canvas measured in caller-selected display units.
pub type CanvasPoint = Point<CanvasSpace>;
/// A window's lower-left screen position in logical points.
pub type FinderScreenPoint = Point<FinderScreenSpace>;

impl<Space> Point<Space> {
	/// Creates a point from its horizontal and vertical components.
	/// # Errors
	/// Returns an error for a non-finite component.
	pub fn new(x: f64, y: f64) -> Result<Self, GeometryError> {
		if !(x.is_finite() && y.is_finite()) {
			return Err(GeometryError::NonFinite);
		}
		Ok(Self {
			x,
			y,
			space: PhantomData,
		})
	}

	#[must_use]
	pub const fn x(&self) -> f64 {
		self.x
	}

	#[must_use]
	pub const fn y(&self) -> f64 {
		self.y
	}
}

/// A positive width and height in the units of their surrounding space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
	width: f64,
	height: f64,
}

impl Size {
	/// Creates a positive finite extent.
	/// # Errors
	/// Rejects non-finite or non-positive dimensions.
	pub fn new(width: f64, height: f64) -> Result<Self, GeometryError> {
		if !(positive_finite(width) && positive_finite(height)) {
			return Err(GeometryError::Size);
		}
		Ok(Self { width, height })
	}

	#[must_use]
	pub const fn width(self) -> f64 {
		self.width
	}

	#[must_use]
	pub const fn height(self) -> f64 {
		self.height
	}
}

/// Converts a Finder lower-left window origin to its top-left point in a
/// downwards screen coordinate system at one unit per logical point.
/// `screen_lower_left` names the shared reference corner in that system.
/// # Errors
/// Returns an error when the converted point cannot be represented finitely.
pub fn window_top_left(
	finder_origin: FinderScreenPoint,
	window_size: Size,
	screen_lower_left: CanvasPoint,
) -> Result<CanvasPoint, GeometryError> {
	CanvasPoint::new(
		screen_lower_left.x + finder_origin.x,
		(screen_lower_left.y - finder_origin.y) - window_size.height,
	)
}

/// Half-open folder rectangle with a top-left minimum and bottom-right maximum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FolderRect {
	min: FolderPoint,
	max: FolderPoint,
}

impl FolderRect {
	/// Makes a rectangle centered at the given icon position.
	/// # Errors
	/// Rejects overflowing or numerically collapsed edges.
	pub fn centered(center: FolderPoint, size: Size) -> Result<Self, GeometryError> {
		let half_width = size.width / 2.0;
		let half_height = size.height / 2.0;
		Self::new(
			FolderPoint::new(center.x - half_width, center.y - half_height)?,
			FolderPoint::new(center.x + half_width, center.y + half_height)?,
		)
	}

	/// Makes a rectangle from a top-left point and size.
	/// # Errors
	/// Rejects overflowing or numerically collapsed edges.
	pub fn from_top_left(min: FolderPoint, size: Size) -> Result<Self, GeometryError> {
		Self::new(
			min,
			FolderPoint::new(min.x + size.width, min.y + size.height)?,
		)
	}

	fn new(min: FolderPoint, max: FolderPoint) -> Result<Self, GeometryError> {
		if min.x >= max.x || min.y >= max.y {
			return Err(GeometryError::Rectangle);
		}
		Ok(Self { min, max })
	}

	#[must_use]
	pub const fn min(self) -> FolderPoint {
		self.min
	}

	#[must_use]
	pub const fn max(self) -> FolderPoint {
		self.max
	}

	#[must_use]
	pub fn contains(self, point: FolderPoint) -> bool {
		point.x >= self.min.x
			&& point.x < self.max.x
			&& point.y >= self.min.y
			&& point.y < self.max.y
	}

	#[must_use]
	pub fn contains_rect(self, other: Self) -> bool {
		self.contains(other.min) && other.max.x <= self.max.x && other.max.y <= self.max.y
	}

	#[must_use]
	pub fn intersects(self, other: Self) -> bool {
		self.min.x < other.max.x
			&& other.min.x < self.max.x
			&& self.min.y < other.max.y
			&& other.min.y < self.max.y
	}
}

/// Folder points map to a translated, uniformly scaled, scrolled canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
	screen_origin: CanvasPoint,
	folder_origin: FolderPoint,
	scale: f64,
}

impl Viewport {
	/// Maps the visible folder origin to the chosen canvas origin.
	/// `scale` is the number of canvas units per folder point, including zoom
	/// or display density as appropriate for the caller's canvas.
	/// # Errors
	/// Rejects zero, negative, non-finite, or non-invertible scales.
	pub fn new(
		screen_origin: CanvasPoint,
		folder_origin: FolderPoint,
		scale: f64,
	) -> Result<Self, GeometryError> {
		if !positive_finite(scale) || !positive_finite(scale.recip()) {
			return Err(GeometryError::Scale);
		}
		Ok(Self {
			screen_origin,
			folder_origin,
			scale,
		})
	}

	#[must_use]
	pub const fn folder_origin(self) -> FolderPoint {
		self.folder_origin
	}

	/// Projects a folder location into the canvas.
	/// # Errors
	/// Returns an error if the projected point is outside the finite domain.
	pub fn to_canvas(self, point: FolderPoint) -> Result<CanvasPoint, GeometryError> {
		CanvasPoint::new(
			self.scale
				.mul_add(point.x - self.folder_origin.x, self.screen_origin.x),
			self.scale
				.mul_add(point.y - self.folder_origin.y, self.screen_origin.y),
		)
	}

	/// Converts a canvas coordinate back to a folder-local location.
	/// Inverse precision depends on the scale and origin's floating-point range.
	/// # Errors
	/// Returns an error if the recovered point is outside the finite domain.
	pub fn to_folder(self, point: CanvasPoint) -> Result<FolderPoint, GeometryError> {
		FolderPoint::new(
			(point.x - self.screen_origin.x) / self.scale + self.folder_origin.x,
			(point.y - self.screen_origin.y) / self.scale + self.folder_origin.y,
		)
	}
}

/// Explicit point-spacing policy for snapping a dragged icon center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
	origin: FolderPoint,
	spacing: f64,
}

impl Grid {
	/// Creates a square grid whose origin lies at a snap point.
	/// # Errors
	/// Rejects non-finite, zero, or negative spacing.
	pub fn new(origin: FolderPoint, spacing: f64) -> Result<Self, GeometryError> {
		if !positive_finite(spacing) {
			return Err(GeometryError::GridSpacing);
		}
		Ok(Self { origin, spacing })
	}

	/// Snaps to the nearest grid point, rounding ties away from the grid origin.
	/// # Errors
	/// Returns an error for an out-of-range result.
	pub fn snap(self, point: FolderPoint) -> Result<FolderPoint, GeometryError> {
		let snap = |value, origin| {
			let cells: f64 = (value - origin) / self.spacing;
			self.spacing.mul_add(cells.round(), origin)
		};
		FolderPoint::new(snap(point.x, self.origin.x), snap(point.y, self.origin.y))
	}
}

/// A drag gesture that preserves the pointer's folder-local offset from the icon center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
	offset: FolderPoint,
}

impl Drag {
	/// Records where the pointer grabbed the icon.
	/// # Errors
	/// Returns an error if the pointer or offset is outside the finite domain.
	pub fn begin(
		center: FolderPoint,
		pointer: CanvasPoint,
		viewport: Viewport,
	) -> Result<Self, GeometryError> {
		let start = viewport.to_folder(pointer)?;
		let offset = FolderPoint::new(center.x - start.x, center.y - start.y)?;
		Ok(Self { offset })
	}

	/// Returns a new center for the current pointer, optionally snapping it.
	/// Drag computations use the current viewport so a gesture can pan or zoom.
	/// # Errors
	/// Returns an error for out-of-range coordinates or snapping.
	pub fn center_at(
		self,
		pointer: CanvasPoint,
		viewport: Viewport,
		grid: Option<Grid>,
	) -> Result<FolderPoint, GeometryError> {
		let pointer = viewport.to_folder(pointer)?;
		let center = FolderPoint::new(pointer.x + self.offset.x, pointer.y + self.offset.y)?;
		match grid {
			Some(grid) => grid.snap(center),
			None => Ok(center),
		}
	}
}

fn positive_finite(number: f64) -> bool {
	number.is_finite() && number > 0.0
}
