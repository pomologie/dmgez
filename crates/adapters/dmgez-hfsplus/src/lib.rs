//! Adapts the independent HFS+ and HFSX engines to dmgez workflows.
//!
//! The `browse` and `verify` features use only the volume reader. The
//! `build` and `image-edit` features host the image writer on Linux, macOS,
//! and Windows.

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

#[cfg(all(feature = "build", any(target_os = "linux", target_os = "macos", windows)))]
mod hfsplus;
#[cfg(all(feature = "image-edit", any(target_os = "linux", target_os = "macos", windows)))]
pub mod rebuild;

#[cfg(all(feature = "image-edit", any(target_os = "linux", target_os = "macos", windows)))]
pub use hfsplus::HostCopyMetadataLoss;
#[cfg(all(feature = "build", any(target_os = "linux", target_os = "macos", windows)))]
pub use hfsplus::{
	Error, HfsBuildReport, HfsImage, MetadataLoss, PreparedHfsBuild, capabilities, prepare,
	prepare_with_progress,
};
