//! Read-only access to a selected HFS+ or HFSX volume.

#[cfg(feature = "browse")]
pub mod browse;
#[cfg(feature = "verify")]
mod error;
#[cfg(feature = "verify")]
mod verification;

#[cfg(feature = "verify")]
pub use error::{CheckError, CheckFailureKind};
#[cfg(feature = "verify")]
pub use hfsplus_reader::check::{ReachableDataReport, StructuralReport};
#[cfg(feature = "verify")]
pub use verification::{check, check_reachable_data};
