use serde::Serialize;

pub(super) const SCHEMA_VERSION: u16 = 1;

/// Read-only action requested by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
	Inspect,
	Verify,
}

impl Operation {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Inspect => "inspect",
			Self::Verify => "verify",
		}
	}
}

/// Explicit backend selection policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
	Auto,
	Native,
	Rust,
}

impl Backend {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Auto => "auto",
			Self::Native => "native",
			Self::Rust => "rust",
		}
	}
}

/// Backend selected for the operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResolvedBackend {
	Native,
}

impl ResolvedBackend {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Native => "native",
		}
	}
}

/// Admitted release artifact kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ArtifactKind {
	AppBundle,
	DiskImage,
}

impl ArtifactKind {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::AppBundle => "app_bundle",
			Self::DiskImage => "disk_image",
		}
	}
}

/// Outcome of the requested signature operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
	/// The tool could read a signature.
	Present,
	/// The tool validated the signature against the artifact and its own requirements.
	Verified,
	/// The tool identified the artifact as unsigned.
	Unsigned,
	/// The tool could not complete the signature operation successfully.
	Failed,
	/// A requested backend or its system tool was unavailable.
	Unavailable,
}

impl Status {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Present => "present",
			Self::Verified => "verified",
			Self::Unsigned => "unsigned",
			Self::Failed => "failed",
			Self::Unavailable => "unavailable",
		}
	}
}

/// Whether this report includes a platform policy assessment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PolicyAssessment {
	NotPerformed,
}

impl PolicyAssessment {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::NotPerformed => "not_performed",
		}
	}
}

/// Result and bounded diagnostic text for one signature operation.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct Report {
	pub schema_version: u16,
	pub operation: Operation,
	pub requested_backend: Backend,
	pub backend: Option<ResolvedBackend>,
	pub status: Status,
	/// Path for display. On platforms with byte paths this can be lossy.
	pub artifact: String,
	pub kind: Option<ArtifactKind>,
	pub assessment: PolicyAssessment,
	/// Untrusted text from the tool, or a backend availability explanation.
	pub diagnostics: Option<String>,
}
