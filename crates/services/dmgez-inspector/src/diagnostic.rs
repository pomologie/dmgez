use dmgez_udif::Error;
use serde::Serialize;

use crate::{ByteRange, CheckStatus, DiskScheme};

/// Stage at which an inspection or verification encountered an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Stage {
	Structure,
	Payload,
	Filesystem,
}

/// Stable error categories from the ecosystem C6 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
	InvalidFormat,
	Corrupt,
	UnsupportedVariant,
	FeatureNotCompiled,
	BackendUnavailable,
	ResourceLimit,
	SourceChanged,
	Conflict,
	PermissionDenied,
	Io,
	Cancelled,
	VerificationFailed,
}

impl ErrorCategory {
	pub(crate) const fn status(self) -> CheckStatus {
		match self {
			Self::UnsupportedVariant => CheckStatus::Unsupported,
			Self::FeatureNotCompiled
			| Self::BackendUnavailable
			| Self::ResourceLimit
			| Self::PermissionDenied
			| Self::Io
			| Self::Cancelled => CheckStatus::Unavailable,
			Self::InvalidFormat
			| Self::Corrupt
			| Self::SourceChanged
			| Self::Conflict
			| Self::VerificationFailed => CheckStatus::Failed,
		}
	}
}

/// Report-safe details with original address spaces and checksum values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticDetail {
	UnsupportedRun {
		code: u32,
		#[serde(serialize_with = "crate::decimal")]
		logical_sector: u64,
	},
	InvalidZlib {
		#[serde(serialize_with = "crate::decimal")]
		physical_offset_bytes: u64,
		#[serde(serialize_with = "crate::decimal")]
		logical_offset_bytes: u64,
	},
	InvalidLzfse {
		#[serde(serialize_with = "crate::decimal")]
		physical_offset_bytes: u64,
		#[serde(serialize_with = "crate::decimal")]
		logical_offset_bytes: u64,
	},
	InvalidLzma {
		#[serde(serialize_with = "crate::decimal")]
		physical_offset_bytes: u64,
		#[serde(serialize_with = "crate::decimal")]
		logical_offset_bytes: u64,
	},
	ChecksumMismatch {
		expected: u32,
		actual: u32,
	},
	PartitionMap {
		scheme: DiskScheme,
	},
	LogicalRange {
		requested: ByteRange,
		#[serde(serialize_with = "crate::decimal")]
		available_bytes: u64,
	},
	/// Byte request relative to the admitted filesystem data window.
	FilesystemWindowRange {
		requested: ByteRange,
		#[serde(serialize_with = "crate::decimal")]
		available_bytes: u64,
	},
	VolumeSelection {
		candidates: usize,
	},
}

/// First condition preventing the requested operation from continuing.
/// Serialized diagnostics omit host error strings, resource values and paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic {
	pub stage: Stage,
	pub category: ErrorCategory,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub detail: Option<DiagnosticDetail>,
}

impl Diagnostic {
	pub(crate) fn from_udif(stage: Stage, error: &Error) -> Self {
		let (category, detail) = match *error {
			Error::InvalidFormat(_) => (ErrorCategory::InvalidFormat, None),
			Error::Unsupported(_) => (ErrorCategory::UnsupportedVariant, None),
			Error::UnsupportedRun { kind, sector } => (
				ErrorCategory::UnsupportedVariant,
				Some(DiagnosticDetail::UnsupportedRun {
					code: kind,
					logical_sector: sector,
				}),
			),
			Error::FeatureNotCompiled(_) => (ErrorCategory::FeatureNotCompiled, None),
			Error::InvalidZlib {
				offset,
				logical_offset,
			} => (
				ErrorCategory::Corrupt,
				Some(DiagnosticDetail::InvalidZlib {
					physical_offset_bytes: offset,
					logical_offset_bytes: logical_offset,
				}),
			),
			Error::InvalidLzfse {
				offset,
				logical_offset,
			} => (
				ErrorCategory::Corrupt,
				Some(DiagnosticDetail::InvalidLzfse {
					physical_offset_bytes: offset,
					logical_offset_bytes: logical_offset,
				}),
			),
			Error::InvalidLzma {
				offset,
				logical_offset,
			} => (
				ErrorCategory::Corrupt,
				Some(DiagnosticDetail::InvalidLzma {
					physical_offset_bytes: offset,
					logical_offset_bytes: logical_offset,
				}),
			),
			Error::ResourceLimit(_) => (ErrorCategory::ResourceLimit, None),
			Error::SourceChanged => (ErrorCategory::SourceChanged, None),
			Error::Cancelled => (ErrorCategory::Cancelled, None),
			Error::ChecksumMismatch {
				expected, actual, ..
			} => (
				ErrorCategory::VerificationFailed,
				Some(DiagnosticDetail::ChecksumMismatch { expected, actual }),
			),
			Error::Io(ref error) => {
				let category = if error.kind() == std::io::ErrorKind::PermissionDenied {
					ErrorCategory::PermissionDenied
				} else {
					ErrorCategory::Io
				};
				(category, None)
			}
			_ => (ErrorCategory::Corrupt, None),
		};
		Self {
			stage,
			category,
			detail,
		}
	}
}
