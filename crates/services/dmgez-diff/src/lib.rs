//! Bounded comparisons of immutable disk sources and selected filesystem views.

mod bytes;
pub mod semantic;

pub use bytes::{DEFAULT_COMPARE_BYTES, Error, Outcome, Report, SCHEMA_VERSION, Side, compare};
