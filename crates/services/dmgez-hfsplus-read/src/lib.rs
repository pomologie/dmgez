//! Read-only access to a selected HFS+ or HFSX volume.

#[cfg(feature = "analysis")]
pub mod analysis;
#[cfg(feature = "browse")]
pub mod browse;
#[cfg(feature = "verify")]
mod error;
#[cfg(feature = "preview")]
pub mod preview;
#[cfg(feature = "verify")]
mod verification;

#[cfg(feature = "verify")]
pub use error::{CheckError, CheckFailureKind};
#[cfg(feature = "verify")]
pub use hfsplus_reader::check::{AlternateStatus, ReachableDataReport, StructuralReport};
#[cfg(feature = "verify")]
pub use verification::{check, check_reachable_data};
