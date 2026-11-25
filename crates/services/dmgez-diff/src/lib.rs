//! Bounded exact comparison of two immutable sources in the same address space.

mod bytes;

pub use bytes::{DEFAULT_COMPARE_BYTES, Error, Outcome, Report, SCHEMA_VERSION, Side, compare};
