//! Headless folder and canvas geometry for dmgez preview consumers.
//!
//! [`ProjectFolder`] is the design-side adapter. The coordinate transforms
//! can also be used on icon locations obtained from a read-only image view.

mod declared;
mod geometry;
mod project;

pub use declared::{DeclaredEntry, DeclaredTree, DeclaredTreeError};
pub use geometry::{
	CanvasPoint, CanvasSpace, Drag, FinderScreenPoint, FinderScreenSpace, FolderPoint, FolderRect,
	FolderSpace, GeometryError, Grid, Point, Size, Viewport, window_top_left,
};
pub use project::{
	FolderStyle, ItemInput, ItemVisibility, LayoutError, PlacedItem, PositionSource, ProjectFolder,
};
