//! Explicit, read-only code signature inspection and validation.
//!
//! Results concern code signatures. Native system policy is a separate check.

#[cfg(all(feature = "native", target_os = "macos"))]
mod native;
mod report;
mod signature;

pub use report::{
	ArtifactKind, Backend, Operation, PolicyAssessment, Report, ResolvedBackend, Status,
};
pub use signature::{ArtifactProblem, Error, check_signature};
