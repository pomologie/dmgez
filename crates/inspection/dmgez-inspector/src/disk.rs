use std::{fmt, num::NonZeroU32};

use dmgez_udif::reader::{Image, ReadAt};
use dmgez_vfs::{Disk, Error as DiskError, Filesystem, Layout, Partition, PartitionKind, Scheme};
use serde::{Serialize, Serializer};

use crate::{
	ByteRange, Diagnostic, DiagnosticDetail, ErrorCategory, InspectionReport, RunWindow, Stage,
	inspect,
};

pub use dmgez_vfs::Limits as DiskLimits;

/// Scheme identifier for the discovered logical disk map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum DiskScheme {
	Apm,
	Gpt,
	Mbr,
	Unreported,
}

impl DiskScheme {
	/// Stable spelling for reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Apm => "apm",
			Self::Gpt => "gpt",
			Self::Mbr => "mbr",
			Self::Unreported => "unreported",
		}
	}
}

impl From<Scheme> for DiskScheme {
	fn from(scheme: Scheme) -> Self {
		match scheme {
			Scheme::Apple => Self::Apm,
			Scheme::Gpt => Self::Gpt,
			Scheme::Mbr => Self::Mbr,
			_ => Self::Unreported,
		}
	}
}

/// Selected record identity in the reported map. Slots start at one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PartitionIdentifier {
	pub scheme: DiskScheme,
	pub slot: NonZeroU32,
}

/// The function assigned to a partition entry by the admitted map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PartitionRole {
	Data,
	Free,
	Metadata,
	ExtendedContainer,
	Unreported,
}

impl From<dmgez_vfs::PartitionRole> for PartitionRole {
	fn from(role: dmgez_vfs::PartitionRole) -> Self {
		match role {
			dmgez_vfs::PartitionRole::Data => Self::Data,
			dmgez_vfs::PartitionRole::Free => Self::Free,
			dmgez_vfs::PartitionRole::Metadata => Self::Metadata,
			dmgez_vfs::PartitionRole::ExtendedContainer => Self::ExtendedContainer,
			_ => Self::Unreported,
		}
	}
}

/// A GUID reported by a GPT entry, in standard UUID byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionGuid(pub [u8; 16]);

impl fmt::Display for PartitionGuid {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		crate::schema::uuid(&self.0, formatter)
	}
}

impl Serialize for PartitionGuid {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

/// Format-specific fields observed in the partition table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartitionDetails {
	Apm {
		name: String,
		type_code: String,
		status: u32,
	},
	Gpt {
		name: String,
		type_guid: PartitionGuid,
		unique_guid: PartitionGuid,
		#[serde(serialize_with = "crate::schema::decimal")]
		attributes: u64,
	},
	Mbr {
		type_code: u8,
		bootable: bool,
	},
	Unreported,
}

impl From<&PartitionKind> for PartitionDetails {
	fn from(kind: &PartitionKind) -> Self {
		match kind {
			PartitionKind::Apple {
				name,
				type_code,
				status,
			} => Self::Apm {
				name: name.clone(),
				type_code: type_code.clone(),
				status: *status,
			},
			PartitionKind::Gpt {
				name,
				type_guid,
				unique_guid,
				attributes,
			} => Self::Gpt {
				name: name.clone(),
				type_guid: PartitionGuid(*type_guid),
				unique_guid: PartitionGuid(*unique_guid),
				attributes: *attributes,
			},
			PartitionKind::Mbr {
				type_code,
				bootable,
			} => Self::Mbr {
				type_code: *type_code,
				bootable: *bootable,
			},
			_ => Self::Unreported,
		}
	}
}

/// One admitted map entry. Both ranges use decoded logical disk bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct PartitionReport {
	pub id: PartitionIdentifier,
	pub role: PartitionRole,
	pub logical: ByteRange,
	/// The window eligible for a filesystem probe, when the map declares one.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub logical_data: Option<ByteRange>,
	pub details: PartitionDetails,
}

impl PartitionReport {
	fn admitted(partition: &Partition) -> Self {
		Self {
			id: PartitionIdentifier {
				scheme: partition.id.scheme.into(),
				slot: partition.id.slot,
			},
			role: partition.role.into(),
			logical: ByteRange::admitted(&partition.range),
			logical_data: partition.data_range.as_ref().map(ByteRange::admitted),
			details: (&partition.kind).into(),
		}
	}
}

/// Whole-disk filesystem recognized by the geometry probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RecognizedFilesystem {
	HfsPlus,
	#[serde(rename = "hfsx")]
	HfsX,
	Unreported,
}

impl From<Filesystem> for RecognizedFilesystem {
	fn from(filesystem: Filesystem) -> Self {
		match filesystem {
			Filesystem::HfsPlus => Self::HfsPlus,
			Filesystem::HfsX => Self::HfsX,
			_ => Self::Unreported,
		}
	}
}

/// Result of requested logical disk discovery, independent of verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiskReport {
	Partitioned {
		scheme: DiskScheme,
		logical_block_bytes: u64,
		partitions: Vec<PartitionReport>,
	},
	Partitionless {
		filesystem: RecognizedFilesystem,
		logical: ByteRange,
	},
	Unknown,
	Unresolved {
		diagnostic: Diagnostic,
	},
}

impl DiskReport {
	fn admitted(layout: &Layout) -> Self {
		match layout {
			Layout::Partitioned {
				scheme,
				block_bytes,
				partitions,
			} => Self::Partitioned {
				scheme: (*scheme).into(),
				logical_block_bytes: *block_bytes,
				partitions: partitions.iter().map(PartitionReport::admitted).collect(),
			},
			Layout::Partitionless { filesystem, range } => Self::Partitionless {
				filesystem: (*filesystem).into(),
				logical: ByteRange::admitted(range),
			},
			Layout::Unknown => Self::Unknown,
			_ => Self::Unresolved {
				diagnostic: Diagnostic {
					stage: Stage::Structure,
					category: ErrorCategory::UnsupportedVariant,
					detail: None,
				},
			},
		}
	}
}

/// Reports the admitted UDIF metadata and probes the logical disk layout.
/// Failed discovery retains the container observations and any run page.
/// Probe bytes are bounded by `limits` on the decoded disk. The reader can
/// decode covering runs to satisfy these reads under its separate limits.
#[must_use]
pub fn inspect_disk<S: ReadAt>(
	image: &mut Image<S>,
	window: Option<RunWindow>,
	limits: DiskLimits,
) -> InspectionReport {
	let mut report = inspect(image, window);
	report.disk = Some(match Disk::discover(image, limits) {
		Ok(disk) => DiskReport::admitted(disk.layout()),
		Err(error) => DiskReport::Unresolved {
			diagnostic: discovery_error(&error),
		},
	});
	report
}

pub(crate) fn discovery_error(error: &DiskError) -> Diagnostic {
	let (category, detail) = match *error {
		DiskError::Source(ref error) => return Diagnostic::from_udif(Stage::Payload, error),
		DiskError::Corrupt { scheme, .. } => (
			ErrorCategory::Corrupt,
			Some(DiagnosticDetail::PartitionMap {
				scheme: scheme.into(),
			}),
		),
		DiskError::Unsupported { scheme, .. } => (
			ErrorCategory::UnsupportedVariant,
			Some(DiagnosticDetail::PartitionMap {
				scheme: scheme.into(),
			}),
		),
		DiskError::Conflict { .. } => (ErrorCategory::Conflict, None),
		DiskError::AmbiguousVolumes { candidates } => (
			ErrorCategory::Conflict,
			Some(DiagnosticDetail::VolumeSelection { candidates }),
		),
		DiskError::UnknownVolume(_) | DiskError::NoDataWindow(_) => (ErrorCategory::Conflict, None),
		DiskError::NoVolume => (ErrorCategory::UnsupportedVariant, None),
		DiskError::Limit(_) => (ErrorCategory::ResourceLimit, None),
		DiskError::OutOfRange {
			offset,
			len,
			available,
		} => (
			ErrorCategory::Corrupt,
			Some(DiagnosticDetail::LogicalRange {
				requested: ByteRange {
					offset_bytes: offset,
					length_bytes: len,
				},
				available_bytes: available,
			}),
		),
		_ => (ErrorCategory::Corrupt, None),
	};
	Diagnostic {
		stage: Stage::Structure,
		category,
		detail,
	}
}
