use std::fmt;

use dmgez_udif::{
	Error,
	reader::{Image, ReadAt},
};
use dmgez_vfs::{Disk, Error as DiskError, LogicalDisk, VolumeId, Window};
use serde::{Serialize, Serializer};

use crate::{
	ByteRange, Diagnostic, DiagnosticDetail, DiskLimits, ErrorCategory, PartitionIdentifier, Stage,
	disk::discovery_error,
};

const DEFAULT_MAX_READ_REQUESTS: u64 = 1_000_000;
const DEFAULT_MAX_READ_BYTES: u64 = 1 << 32;

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
	/// Volume structure was checked. File payloads and other full checks may be absent.
	VolumeStructure,
	/// All required filesystem checks passed within the reported scope.
	FullVolume,
}

/// Engine-native object identifier scoped to the selected filesystem view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemObjectId(pub u64);

impl Serialize for FilesystemObjectId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		crate::schema::decimal(&self.0, serializer)
	}
}

/// Successful checking evidence for a data window or one engine-native volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FilesystemEvidence {
	pub scope: FilesystemScope,
	/// An ID narrows assurance to one native volume inside the data window.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub volume_object_id: Option<FilesystemObjectId>,
}

impl FilesystemEvidence {
	/// Covers the filesystem content of the selected data window at this scope.
	#[must_use]
	pub const fn window(scope: FilesystemScope) -> Self {
		Self {
			scope,
			volume_object_id: None,
		}
	}

	/// Covers only one native volume within the selected data window.
	#[must_use]
	pub const fn volume(scope: FilesystemScope, id: FilesystemObjectId) -> Self {
		Self {
			scope,
			volume_object_id: Some(id),
		}
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
	fn category(category: ErrorCategory) -> Self {
		Self {
			diagnostic: Diagnostic {
				stage: Stage::Filesystem,
				category,
				detail: None,
			},
			cause: None,
		}
	}

	fn disk(error: DiskError) -> Self {
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

pub(crate) fn verify_window<S: ReadAt>(
	image: &mut Image<S>,
	options: FilesystemOptions,
	checker: impl FnOnce(&mut dyn FilesystemSource) -> Result<FilesystemEvidence, FilesystemCheckError>,
) -> Result<Attempt, Failure> {
	let mut disk = Disk::discover(image, options.disk_limits).map_err(Failure::disk)?;
	let selected = disk.select_volume(options.volume).map_err(Failure::disk)?;
	let sole_window = disk.select_volume(None).is_ok_and(|only| only == selected);
	let id = SelectedWindowId::from_volume(selected)
		.ok_or_else(|| Failure::category(ErrorCategory::UnsupportedVariant))?;
	let window = disk.data_window(selected).map_err(Failure::disk)?;
	let selected = SelectedWindow {
		id,
		logical: ByteRange {
			offset_bytes: window.logical_offset(),
			length_bytes: window.len(),
		},
	};
	let mut source = BudgetedSource {
		window,
		limits: options,
		charged_requests: 0,
		charged_bytes: 0,
		failure: None,
	};
	let checked = checker(&mut source);
	let result = match source.failure.take() {
		Some((_, failure)) => Err(failure),
		None => checked
			.map_err(|error| Failure::category(error.category()))
			.and_then(|evidence| {
				// Account this independent identity check outside the engine's budget.
				let end = source.window.len();
				source
					.window
					.read_exact_at(end, &mut [])
					.map_err(Failure::disk)?;
				Ok(evidence)
			}),
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
			charged_read_requests: source.charged_requests,
			charged_read_bytes: source.charged_bytes,
		},
		failure,
	})
}

struct BudgetedSource<'source, S> {
	window: Window<'source, S>,
	limits: FilesystemOptions,
	charged_requests: u64,
	charged_bytes: u64,
	failure: Option<(FilesystemReadError, Failure)>,
}

impl<S: LogicalDisk> FilesystemSource for BudgetedSource<'_, S> {
	fn len(&self) -> u64 {
		self.window.len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), FilesystemReadError> {
		if let Some((error, _)) = &self.failure {
			return Err(*error);
		}
		let length = output.len() as u64;
		let Some(bytes) = self
			.charged_bytes
			.checked_add(length)
			.filter(|&bytes| bytes <= self.limits.max_read_bytes)
		else {
			return self.reject(
				FilesystemReadError::Budget,
				Failure::category(ErrorCategory::ResourceLimit),
			);
		};
		if self.charged_requests == self.limits.max_read_requests {
			return self.reject(
				FilesystemReadError::Budget,
				Failure::category(ErrorCategory::ResourceLimit),
			);
		}
		self.charged_bytes = bytes;
		self.charged_requests += 1;
		self.window.read_exact_at(offset, output).or_else(|error| {
			if let DiskError::OutOfRange {
				offset,
				len,
				available,
			} = error
			{
				let mut failure = Failure::category(ErrorCategory::Corrupt);
				failure.diagnostic.detail = Some(DiagnosticDetail::FilesystemWindowRange {
					requested: ByteRange {
						offset_bytes: offset,
						length_bytes: len,
					},
					available_bytes: available,
				});
				self.reject(FilesystemReadError::OutOfRange, failure)
			} else {
				self.reject(FilesystemReadError::Source, Failure::disk(error))
			}
		})
	}
}

impl<S> BudgetedSource<'_, S> {
	fn reject(
		&mut self,
		error: FilesystemReadError,
		failure: Failure,
	) -> Result<(), FilesystemReadError> {
		self.failure = Some((error, failure));
		Err(error)
	}
}
