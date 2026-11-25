//! Integration witness for the HFS+ C5 filesystem view.

pub use dmgez_hfsplus::browse::{
	Error, HfsBackend, HfsDirectory, HfsView, Limits, SourceError, with_view,
};
