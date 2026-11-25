use serde::{Serialize, Serializer};

/// Version of the serialized report shapes.
pub const SCHEMA_VERSION: u32 = 1;

/// Operation identified by the report envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ReportOperation {
	Inspect,
	Verify,
	Analyze,
}

/// Layers supported by this report service. Availability of individual run
/// decoders is reported on the admitted container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Capabilities {
	pub structure: bool,
	pub container_checksums: bool,
	pub filesystem: bool,
	pub security: bool,
}

pub(super) const CAPABILITIES: Capabilities = Capabilities {
	structure: true,
	container_checksums: true,
	filesystem: false,
	security: false,
};

/// Result of an individual verification check or layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
	NotChecked,
	Passed,
	Partial,
	Failed,
	NotPresent,
	Unsupported,
	Unavailable,
}

impl CheckStatus {
	/// Stable spelling used in machine-readable reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::NotChecked => "not_checked",
			Self::Passed => "passed",
			Self::Partial => "partial",
			Self::Failed => "failed",
			Self::NotPresent => "not_present",
			Self::Unsupported => "unsupported",
			Self::Unavailable => "unavailable",
		}
	}
}

/// Independent verdicts for each layer of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Coverage {
	pub structure: CheckStatus,
	/// Decoded UDIF payload and declared checksum domains.
	pub payload: CheckStatus,
	pub filesystem: CheckStatus,
	/// Signature or trust checks, separate from all CRC checks.
	pub security: CheckStatus,
}

impl Default for Coverage {
	fn default() -> Self {
		Self {
			structure: CheckStatus::NotChecked,
			payload: CheckStatus::NotChecked,
			filesystem: CheckStatus::NotChecked,
			security: CheckStatus::NotChecked,
		}
	}
}

pub(super) fn decimal<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.collect_str(value)
}
