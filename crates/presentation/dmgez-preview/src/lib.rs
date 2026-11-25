//! Headless folder and canvas geometry for dmgez preview consumers.
//!
//! [`ProjectFolder`] is the design-side adapter. The coordinate transforms
//! can also be used on icon locations obtained from a read-only image view.

#[cfg(feature = "actual")]
pub mod actual;
mod declared;
mod geometry;
#[cfg(feature = "png-decode")]
pub mod png_image;
mod project;
#[cfg(feature = "png-render")]
pub mod raster;

pub use declared::{DeclaredEntry, DeclaredTree, DeclaredTreeError};
pub use geometry::{
	CanvasPoint, CanvasSpace, Drag, FinderScreenPoint, FinderScreenSpace, FolderPoint, FolderRect,
	FolderSpace, GeometryError, Grid, Point, Size, Viewport, window_top_left,
};
pub use project::{
	FolderStyle, ItemInput, ItemVisibility, LayoutError, PlacedItem, PositionSource, ProjectFolder,
};
