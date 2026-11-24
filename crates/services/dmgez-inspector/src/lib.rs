//! Versioned reports for the bounded, single-segment UDIF read profile.
//!
//! Inspection reports container metadata and can probe the logical disk layout.
//! Verification checks the requested scope and records unavailable checks.

mod container;
mod diagnostic;
mod disk;
mod verification;

use serde::{Serialize, Serializer};

pub use container::{
	ByteRange, ChecksumDeclaration, ContainerFormat, ContainerSummary, InspectionReport,
	MAX_RUN_PAGE_SIZE, Run, RunEncoding, RunPage, RunSummary, RunWindow, SegmentId, inspect,
};
pub use diagnostic::{Diagnostic, DiagnosticDetail, ErrorCategory, Stage};
pub use disk::{
	DiskLimits, DiskReport, DiskScheme, PartitionDetails, PartitionGuid, PartitionIdentifier,
	PartitionReport, PartitionRole, RecognizedFilesystem, inspect_disk,
};
pub use verification::{
	ChecksumChecks, VerificationMode, VerificationOutcome, VerificationReport, verify,
};

/// Version of the serialized report shapes.
pub const SCHEMA_VERSION: u32 = 1;

/// Operation identified by the report envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ReportOperation {
	Inspect,
	Verify,
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

const CAPABILITIES: Capabilities = Capabilities {
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

fn decimal<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.collect_str(value)
}
