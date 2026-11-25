//! Explicit native policy checks for release artifacts.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::ArtifactKind;

pub(super) const ASSESSMENT_SCHEMA_VERSION: u16 = 1;

/// Native tool selected by artifact kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeAssessmentTool {
	Spctl,
	SyspolicyCheck,
}

impl NativeAssessmentTool {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Spctl => "spctl",
			Self::SyspolicyCheck => "syspolicy_check",
		}
	}
}

/// Reason no affirmative or negative native verdict could be obtained.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeAssessmentUnavailable {
	BackendUnavailable,
	MissingTool,
	PolicyDisabled,
	OverriddenVerdict,
	UnknownToolResponse,
}

impl NativeAssessmentUnavailable {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::BackendUnavailable => "backend_unavailable",
			Self::MissingTool => "missing_tool",
			Self::PolicyDisabled => "policy_disabled",
			Self::OverriddenVerdict => "overridden_verdict",
			Self::UnknownToolResponse => "unknown_tool_response",
		}
	}
}

/// The result of evaluating a release artifact under the local macOS policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeAssessmentStatus {
	Accepted,
	Rejected,
	Unavailable(NativeAssessmentUnavailable),
}

impl NativeAssessmentStatus {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Accepted => "accepted",
			Self::Rejected => "rejected",
			Self::Unavailable(_) => "unavailable",
		}
	}
}

/// Native policy evidence for one assessment on the current host.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct NativeAssessmentReport {
	pub schema_version: u16,
	pub artifact: String,
	pub kind: Option<ArtifactKind>,
	pub tool: Option<NativeAssessmentTool>,
	#[serde(flatten)]
	pub outcome: NativeAssessmentStatus,
}

/// Failure to inspect an input or run the selected policy tool.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NativeAssessmentError {
	#[error(transparent)]
	Artifact(#[from] crate::Error),
	#[error("could not run the native assessment tool")]
	Tool(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Asks macOS to assess the supplied `.app` or UDIF `.dmg` under local policy.
///
/// Keep the artifact stable throughout the call, including nested app files.
/// The operating system may use its trust services during this explicit
/// check. The operation is independent of signature and ticket validation,
/// and does not launch an app, mount an image or modify security settings.
///
/// # Errors
///
/// Returns an error for an invalid or inaccessible artifact, an observed
/// source change, or a failure to run or capture the assessment tool.
pub fn assess_native_policy(path: &Path) -> Result<NativeAssessmentReport, NativeAssessmentError> {
	#[cfg(all(feature = "native", target_os = "macos"))]
	{
		crate::native::assessment::check(path)
	}
	#[cfg(not(all(feature = "native", target_os = "macos")))]
	{
		Ok(NativeAssessmentReport {
			schema_version: ASSESSMENT_SCHEMA_VERSION,
			artifact: path.to_string_lossy().into_owned(),
			kind: None,
			tool: None,
			outcome: NativeAssessmentStatus::Unavailable(
				NativeAssessmentUnavailable::BackendUnavailable,
			),
		})
	}
}
