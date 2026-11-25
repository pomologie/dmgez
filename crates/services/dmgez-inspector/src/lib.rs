//! Versioned reports for the bounded, single-segment UDIF read profile.
//!
//! Inspection reports container metadata and can probe the logical disk layout.
//! Verification checks the requested scope and records unavailable checks.

mod analysis;
mod container;
mod diagnostic;
mod disk;
mod filesystem;
#[cfg(feature = "remote")]
pub mod remote;
mod schema;
mod verification;
pub mod volume_queries;

pub use analysis::{AnalysisReport, RunSpace, RunSpaceByEncoding, analyze};
pub use container::{
	ByteRange, ChecksumDeclaration, ContainerFormat, ContainerSummary, InspectionReport,
	MAX_RUN_PAGE_SIZE, Run, RunEncoding, RunPage, RunSummary, RunWindow, SegmentId, inspect,
};
pub use diagnostic::{Diagnostic, DiagnosticDetail, ErrorCategory, Stage};
pub use disk::{
	DiskLimits, DiskReport, DiskScheme, PartitionDetails, PartitionGuid, PartitionIdentifier,
	PartitionReport, PartitionRole, RecognizedFilesystem, inspect_disk,
};
pub use filesystem::{
	FilesystemCheckError, FilesystemCheckReport, FilesystemCheckpointDiagnostics,
	FilesystemCheckpointId, FilesystemEvidence, FilesystemObjectId, FilesystemOptions,
	FilesystemProbe, FilesystemReadError, FilesystemScope, FilesystemSource, FilesystemUuid,
	SelectedWindow, SelectedWindowId,
};
pub use schema::{Capabilities, CheckStatus, Coverage, ReportOperation, SCHEMA_VERSION};
pub use verification::{
	ChecksumChecks, VerificationMode, VerificationOutcome, VerificationReport, verify,
	verify_filesystem, verify_filesystem_with_probe, verify_raw_filesystem,
};
