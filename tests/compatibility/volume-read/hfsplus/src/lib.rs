//! HFS+ read-only adapter for the C5 filesystem view.

mod volume;

pub use volume::{Error, HfsBackend, HfsDirectory, HfsView, Limits, SourceError, with_view};
