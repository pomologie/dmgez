//! Explicit code signing and signature checks.
//!
//! Results concern code signatures. Native system policy is a separate check.

#[cfg(all(feature = "native", target_os = "macos"))]
mod native;
mod report;
mod signature;
mod signing;

pub use report::{
	ArtifactKind, Backend, Operation, PolicyAssessment, Report, ResolvedBackend, Status,
};
pub use signature::{ArtifactProblem, Error, check_signature};
pub use signing::{DiskSigningReport, SignError, sign_disk_image};
