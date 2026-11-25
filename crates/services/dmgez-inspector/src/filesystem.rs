use std::{
	fmt,
	num::{NonZeroU32, NonZeroU64},
};

use dmgez_udif::Error;
use dmgez_vfs::{Disk, Error as DiskError, Filesystem, Layout, LogicalDisk, PartitionId, VolumeId};
use serde::{Serialize, Serializer};

use crate::{
	ByteRange, Diagnostic, DiagnosticDetail, DiskLimits, ErrorCategory, PartitionIdentifier, Stage,
	disk::discovery_error,
};

const DEFAULT_MAX_READ_REQUESTS: u64 = 1_000_000;
const DEFAULT_MAX_READ_BYTES: u64 = 1 << 32;

mod budget;

pub(crate) use budget::ReadBudget;

/// Selection and aggregate input limits for one filesystem verification.
#[derive(Debug, Clone, Copy)]
pub struct FilesystemOptions {
	pub disk_limits: DiskLimits,
	/// An explicit window selector. Without one, exactly one data window is required.
	pub volume: Option<VolumeId>,
	/// Maximum reads made by the checker, including empty reads.
	pub max_read_requests: u64,
	/// Maximum combined decoded bytes requested by the checker.
	pub max_read_bytes: u64,
}

impl Default for FilesystemOptions {
	fn default() -> Self {
		Self {
			disk_limits: DiskLimits::default(),
			volume: None,
			max_read_requests: DEFAULT_MAX_READ_REQUESTS,
			max_read_bytes: DEFAULT_MAX_READ_BYTES,
		}
	}
}

/// Optional whole-disk recognizer and partition selection for a filesystem checker.
///
/// The recognizer runs only if disk discovery found no partition map or
/// whole-disk filesystem. It must return the nonzero byte length of a validated
/// filesystem extent starting at zero, or `None` for unrecognized input.
pub struct FilesystemProbe<P> {
	filesystem: Filesystem,
	partition_slot: Option<NonZeroU32>,
	probe: P,
}

impl<P> FilesystemProbe<P>
where
	P: FnOnce(&mut dyn FilesystemSource) -> Result<Option<NonZeroU64>, FilesystemCheckError>,
{
	#[must_use]
	pub const fn new(filesystem: Filesystem, partition_slot: Option<NonZeroU32>, probe: P) -> Self {
		Self {
			filesystem,
			partition_slot,
			probe,
		}
	}

	pub(crate) fn select<S: LogicalDisk>(
		self,
		disk: &mut Disk<S>,
		requested: Option<VolumeId>,
		budget: &mut ReadBudget,
		recognition_stage: Stage,
	) -> Result<Option<VolumeId>, Failure> {
		if let Some(slot) = self.partition_slot {
			let Layout::Partitioned { scheme, .. } = *disk.layout() else {
				return Err(Failure::category(ErrorCategory::Conflict));
			};
			if requested.is_some() {
				return Err(Failure::category(ErrorCategory::Conflict));
			}
			return Ok(Some(VolumeId::Partition(PartitionId { scheme, slot })));
		}
		disk.recognize_partitionless(self.filesystem, |window| {
			let mut source = budget.source(window);
			let result = (self.probe)(&mut source);
			source.finish(result)
		})
		.map_err(|failure| failure.at(recognition_stage))?;
		Ok(requested)
	}
}

/// Read-only positional access to the selected filesystem data extent.
pub trait FilesystemSource {
	fn len(&self) -> u64;

	fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Reads exactly within the selected extent and enforces the aggregate budget.
	///
	/// # Errors
	/// Reports an invalid range, exhausted input budget, or source failure.
	/// The verification attempt remains failed even if the checker suppresses it.
	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), FilesystemReadError>;
}

/// Reason a checker read could not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilesystemReadError {
	Budget,
	OutOfRange,
	Source,
}

impl fmt::Display for FilesystemReadError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Budget => "filesystem read budget exceeded",
			Self::OutOfRange => "read exceeds the selected filesystem window",
			Self::Source => "filesystem source read failed",
		})
	}
}

impl std::error::Error for FilesystemReadError {}

impl FilesystemReadError {
	const fn category(self) -> ErrorCategory {
		match self {
			Self::Budget => ErrorCategory::ResourceLimit,
			Self::OutOfRange => ErrorCategory::Corrupt,
			Self::Source => ErrorCategory::Io,
		}
	}
}

/// Typed stopper reported by a filesystem engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilesystemCheckError {
	Unsupported,
	Corrupt,
	ResourceLimit,
	Unavailable,
	Cancelled,
	/// The requested native volume or checkpoint could not be selected.
	Selection,
	/// Automatic selection requires an explicit native volume selector.
	SelectionRequired {
		candidates: usize,
	},
	Read(FilesystemReadError),
}

impl FilesystemCheckError {
	const fn category(self) -> ErrorCategory {
		match self {
			Self::Unsupported => ErrorCategory::UnsupportedVariant,
			Self::Corrupt => ErrorCategory::Corrupt,
			Self::ResourceLimit => ErrorCategory::ResourceLimit,
			Self::Unavailable => ErrorCategory::BackendUnavailable,
			Self::Cancelled => ErrorCategory::Cancelled,
			Self::Selection | Self::SelectionRequired { .. } => ErrorCategory::Conflict,
			Self::Read(error) => error.category(),
		}
	}
}

impl From<FilesystemReadError> for FilesystemCheckError {
	fn from(error: FilesystemReadError) -> Self {
		Self::Read(error)
	}
}

/// Assurance earned by a successful engine check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum FilesystemScope {
	/// Admitted reachable metadata was checked. Global consistency is unknown.
	ReachableStructure,
	/// Reachable structure and logical payload reads completed.
	ReachableData,
	/// Reachable structure and stored payload reads completed.
	ReachableStoredData,
	/// Volume structure was checked. File payloads and other full checks may be absent.
	VolumeStructure,
	/// All required filesystem checks passed within the reported scope.
	FullVolume,
}

impl FilesystemScope {
	/// Stable spelling for text and JSON reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::ReachableStructure => "reachable_structure",
			Self::ReachableData => "reachable_data",
			Self::ReachableStoredData => "reachable_stored_data",
			Self::VolumeStructure => "volume_structure",
			Self::FullVolume => "full_volume",
		}
	}
}

/// Engine-native object identifier scoped to the selected filesystem view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemObjectId(pub u64);

impl Serialize for FilesystemObjectId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		crate::schema::decimal(&self.0, serializer)
	}
}

/// Transaction identifier of the checked native filesystem checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemCheckpointId(pub u64);

impl Serialize for FilesystemCheckpointId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		crate::schema::decimal(&self.0, serializer)
	}
}

/// Native filesystem volume UUID in standard UUID byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemUuid(pub [u8; 16]);

impl fmt::Display for FilesystemUuid {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		crate::schema::uuid(&self.0, formatter)
	}
}

impl Serialize for FilesystemUuid {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

/// Bounded summary of warnings from native checkpoint selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FilesystemCheckpointDiagnostics {
	pub rejected_descriptors: usize,
	/// A newer XID advertised by the bootstrap than the selected checkpoint.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub advertised_xid: Option<FilesystemCheckpointId>,
}

/// Successful checking evidence for a data window or one engine-native volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FilesystemEvidence {
	pub scope: FilesystemScope,
	/// An ID narrows assurance to one native volume inside the data window.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub volume_object_id: Option<FilesystemObjectId>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub checkpoint_xid: Option<FilesystemCheckpointId>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub volume_uuid: Option<FilesystemUuid>,
	/// Logical payload bytes inspected by the checker, including sparse zero bytes.
	#[serde(
		skip_serializing_if = "Option::is_none",
		serialize_with = "crate::schema::optional_decimal"
	)]
	pub logical_payload_bytes_read: Option<u64>,
	/// Stored fork bytes inspected by the checker, excluding allocation slack.
	#[serde(
		skip_serializing_if = "Option::is_none",
		serialize_with = "crate::schema::optional_decimal"
	)]
	pub stored_payload_bytes_read: Option<u64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub checkpoint_diagnostics: Option<FilesystemCheckpointDiagnostics>,
}

impl FilesystemEvidence {
	/// Covers the filesystem content of the selected data window at this scope.
	#[must_use]
	pub const fn window(scope: FilesystemScope) -> Self {
		Self {
			scope,
			volume_object_id: None,
			checkpoint_xid: None,
			volume_uuid: None,
			logical_payload_bytes_read: None,
			stored_payload_bytes_read: None,
			checkpoint_diagnostics: None,
		}
	}

	/// Covers reachable structure and stored fork reads in the selected window.
	#[must_use]
	pub const fn stored_window(bytes_read: u64) -> Self {
		Self {
			stored_payload_bytes_read: Some(bytes_read),
			..Self::window(FilesystemScope::ReachableStoredData)
		}
	}

	/// Covers only one native volume within the selected data window.
	#[must_use]
	pub const fn volume(scope: FilesystemScope, id: FilesystemObjectId) -> Self {
		Self {
			volume_object_id: Some(id),
			..Self::window(scope)
		}
	}

	/// Covers one native volume and its selected checkpoint.
	#[must_use]
	pub const fn checkpoint(
		scope: FilesystemScope,
		id: FilesystemObjectId,
		checkpoint_xid: FilesystemCheckpointId,
		volume_uuid: FilesystemUuid,
		logical_payload_bytes_read: Option<u64>,
	) -> Self {
		Self {
			volume_object_id: Some(id),
			checkpoint_xid: Some(checkpoint_xid),
			volume_uuid: Some(volume_uuid),
			logical_payload_bytes_read,
			..Self::window(scope)
		}
	}

	/// Records checkpoint scan warnings when the scan emitted any.
	#[must_use]
	pub const fn with_checkpoint_diagnostics(
		mut self,
		rejected_descriptors: usize,
		advertised_xid: Option<FilesystemCheckpointId>,
	) -> Self {
		self.checkpoint_diagnostics = if rejected_descriptors == 0 && advertised_xid.is_none() {
			None
		} else {
			Some(FilesystemCheckpointDiagnostics {
				rejected_descriptors,
				advertised_xid,
			})
		};
		self
	}
}

/// Identity of the window checked within the decoded disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectedWindowId {
	WholeDisk,
	Partition { id: PartitionIdentifier },
}

impl SelectedWindowId {
	/// Converts an admitted disk window identity to its serialized representation.
	#[must_use]
	pub fn from_volume(volume: VolumeId) -> Option<Self> {
		match volume {
			VolumeId::WholeDisk => Some(Self::WholeDisk),
			VolumeId::Partition(id) => Some(Self::Partition {
				id: PartitionIdentifier {
					scheme: id.scheme.into(),
					slot: id.slot,
				},
			}),
			_ => None,
		}
	}
}

/// Admitted filesystem extent expressed in decoded disk bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SelectedWindow {
	pub id: SelectedWindowId,
	pub logical: ByteRange,
}

/// Coverage and input accounting for one selected filesystem window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FilesystemCheckReport {
	pub window: SelectedWindow,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub evidence: Option<FilesystemEvidence>,
	/// The entire image's filesystem layer passed when this field is true.
	pub all_filesystems_checked: bool,
	/// Reads admitted under the budget, including any failed source read.
	/// Includes any reads used to recognize a partitionless filesystem.
	#[serde(serialize_with = "crate::schema::decimal")]
	pub charged_read_requests: u64,
	#[serde(serialize_with = "crate::schema::decimal")]
	pub charged_read_bytes: u64,
}

pub(crate) struct Failure {
	pub diagnostic: Diagnostic,
	pub cause: Option<Error>,
}

impl Failure {
	pub(crate) fn at(mut self, stage: Stage) -> Self {
		self.diagnostic.stage = stage;
		self
	}

	pub(super) fn category(category: ErrorCategory) -> Self {
		Self {
			diagnostic: Diagnostic {
				stage: Stage::Filesystem,
				category,
				detail: None,
			},
			cause: None,
		}
	}

	fn checker(error: FilesystemCheckError) -> Self {
		let mut failure = Self::category(error.category());
		if let FilesystemCheckError::SelectionRequired { candidates } = error {
			failure.diagnostic.detail =
				Some(DiagnosticDetail::FilesystemVolumeSelection { candidates });
		}
		failure
	}
}

impl From<DiskError> for Failure {
	fn from(error: DiskError) -> Self {
		let mut diagnostic = discovery_error(&error);
		diagnostic.stage = Stage::Filesystem;
		let cause = match error {
			DiskError::Source(error) => Some(error),
			_ => None,
		};
		Self { diagnostic, cause }
	}
}

pub(crate) struct Attempt {
	pub report: FilesystemCheckReport,
	pub failure: Option<Failure>,
}

pub(crate) fn verify_window<S: LogicalDisk>(
	disk: &mut Disk<S>,
	requested: Option<VolumeId>,
	mut budget: ReadBudget,
	checker: impl FnOnce(&mut dyn FilesystemSource) -> Result<FilesystemEvidence, FilesystemCheckError>,
) -> Result<Attempt, Failure> {
	let selected = disk.select_volume(requested)?;
	let sole_window = disk.select_volume(None).is_ok_and(|only| only == selected);
	let id = SelectedWindowId::from_volume(selected)
		.ok_or_else(|| Failure::category(ErrorCategory::UnsupportedVariant))?;
	let window = disk.data_window(selected)?;
	let selected = SelectedWindow {
		id,
		logical: ByteRange {
			offset_bytes: window.logical_offset(),
			length_bytes: window.len(),
		},
	};
	let result = {
		let mut source = budget.source(window);
		let checked = checker(&mut source);
		source.finish(checked).and_then(|evidence| {
			source.revalidate()?;
			Ok(evidence)
		})
	};
	let (evidence, failure) = match result {
		Ok(evidence) => (Some(evidence), None),
		Err(error) => (None, Some(error)),
	};
	let all_filesystems_checked = sole_window
		&& evidence.is_some_and(|evidence| {
			evidence.scope == FilesystemScope::FullVolume && evidence.volume_object_id.is_none()
		});
	Ok(Attempt {
		report: FilesystemCheckReport {
			window: selected,
			evidence,
			all_filesystems_checked,
			charged_read_requests: budget.charged_requests,
			charged_read_bytes: budget.charged_bytes,
		},
		failure,
	})
}
