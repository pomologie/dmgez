//! Explicit code signing and signature checks.
//!
//! Results concern code signatures. Native system policy is a separate check.

mod assessment;
#[cfg(all(feature = "native", target_os = "macos"))]
mod native;
#[cfg(feature = "notarization-native")]
pub mod notarization;
mod parameters;
pub mod release_plan;
mod report;
mod signature;
mod signing;

pub use assessment::{
	NativeAssessmentError, NativeAssessmentReport, NativeAssessmentStatus, NativeAssessmentTool,
	NativeAssessmentUnavailable, assess_native_policy,
};
pub use parameters::{
	is_valid_certificate_fingerprint, is_valid_disk_identifier, is_valid_keychain_profile_name,
};
pub use report::{
	ArtifactKind, Backend, Operation, PolicyAssessment, Report, ResolvedBackend, Status,
};
pub use signature::{ArtifactProblem, Error, check_signature};
pub use signing::{
	AppEntitlements, AppSigningItemReport, AppSigningReport, DiskSigningReport, SignError,
	sign_application, sign_disk_image, verify_signed_disk_image,
};
