use std::{cell::RefCell, error::Error as StdError};

use apfs_container::{Error as ContainerError, ObjectMapError, VolumeError};
use apfs_core::{ByteOffset, ReadAt};
use apfs_format::{Error as FormatError, volume::Error as VolumeFormatError};
use apfs_reader::{CheckData, CheckOptions, CheckReport, Corruption, Error as ReaderError};
use thiserror::Error;

use crate::{
	CheckpointDiagnostic, OpenError, TransactionId, VolumeInfo, VolumeSelector,
	container::{open_container, open_volume},
};

#[cfg(test)]
mod tests;

/// Successful checks in one admitted APFS volume and checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VerificationEvidence {
	pub volume: VolumeInfo,
	pub reachable: CheckReport,
	/// Checkpoint scan diagnostics, including evidence of fallback.
	pub checkpoint_diagnostics: Vec<CheckpointDiagnostic>,
}

/// Classification of a failed APFS probe or verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CheckFailureKind {
	Unsupported,
	Corrupt,
	ResourceLimit,
	Selection,
	Read,
	Cancelled,
}

/// Failed APFS verification with the underlying engine or source error.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct CheckError<E: StdError + 'static>(CheckFailure<E>);

impl<E: StdError + 'static> CheckError<E> {
	#[must_use]
	pub fn kind(&self) -> CheckFailureKind {
		match &self.0 {
			CheckFailure::Open(error) => match error {
				OpenError::Container(error) => container_failure(error),
				OpenError::Reader(error) => reader_failure(error),
				OpenError::NoVolumes
				| OpenError::AmbiguousVolumes(_)
				| OpenError::UnknownVolume(_) => CheckFailureKind::Selection,
				OpenError::Allocation => CheckFailureKind::ResourceLimit,
			},
			CheckFailure::Reachable(error) => reader_failure(error),
			CheckFailure::EvidenceAllocation => CheckFailureKind::ResourceLimit,
		}
	}

	/// Number of candidates when a volume selector is required.
	#[must_use]
	pub const fn selection_candidates(&self) -> Option<usize> {
		match &self.0 {
			CheckFailure::Open(OpenError::AmbiguousVolumes(count)) => Some(*count),
			_ => None,
		}
	}
}

/// Checks the records reachable from the root of one selected APFS volume.
///
/// The source must preserve its length and identity across reads. The caller
/// may enforce an aggregate source budget in addition to the engine's
/// checkpoint, tree, and logical payload limits.
///
/// # Errors
/// Fails if selection is ambiguous or unavailable, the source fails, metadata
/// is corrupt or unsupported, a budget is exhausted, or the check is cancelled.
pub fn check<E: StdError + 'static>(
	length: u64,
	read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	data: CheckData,
	mut cancelled: impl FnMut() -> bool,
) -> Result<VerificationEvidence, CheckError<E>> {
	if cancelled() {
		return Err(CheckError(CheckFailure::Reachable(ReaderError::Cancelled)));
	}
	let source = VerificationSource {
		length,
		read: RefCell::new(read),
	};
	let container = open_container(source, xid)
		.map_err(|error| CheckError(CheckFailure::Open(error.into())))?;
	let mut view =
		open_volume(&container, selector).map_err(|error| CheckError(CheckFailure::Open(error)))?;
	let options = CheckOptions {
		data,
		..CheckOptions::default()
	};
	let reachable = view
		.check_reachable(options, cancelled)
		.map_err(|error| CheckError(CheckFailure::Reachable(error)))?;
	let diagnostics = container.diagnostics();
	let mut checkpoint_diagnostics = Vec::new();
	checkpoint_diagnostics
		.try_reserve_exact(diagnostics.len())
		.map_err(|_| CheckError(CheckFailure::EvidenceAllocation))?;
	checkpoint_diagnostics.extend_from_slice(diagnostics);
	Ok(VerificationEvidence {
		volume: VolumeInfo::from(view.volume()),
		reachable,
		checkpoint_diagnostics,
	})
}

#[derive(Debug, Error)]
enum CheckFailure<E: StdError + 'static> {
	#[error(transparent)]
	Open(OpenError<ReadError<E>>),
	#[error(transparent)]
	Reachable(ReaderError<ReadError<E>>),
	#[error("could not allocate APFS verification evidence")]
	EvidenceAllocation,
}

#[derive(Debug, Error)]
enum ReadError<E: StdError + 'static> {
	#[error("APFS verification read exceeds the selected extent")]
	OutOfRange,
	#[error("APFS verification source already has an active positional read")]
	ActiveRead,
	#[error("{0}")]
	Source(#[source] E),
}

struct VerificationSource<F> {
	length: u64,
	read: RefCell<F>,
}

impl<F, E> ReadAt for VerificationSource<F>
where
	F: FnMut(u64, &mut [u8]) -> Result<(), E>,
	E: StdError + 'static,
{
	type Error = ReadError<E>;

	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		if offset
			.0
			.checked_add(output.len() as u64)
			.is_none_or(|end| end > self.length)
		{
			return Err(ReadError::OutOfRange);
		}
		let mut read = self
			.read
			.try_borrow_mut()
			.map_err(|_| ReadError::ActiveRead)?;
		read(offset.0, output).map_err(ReadError::Source)
	}
}

fn container_failure<E: StdError + 'static>(
	error: &ContainerError<ReadError<E>>,
) -> CheckFailureKind {
	match error {
		ContainerError::Bootstrap(FormatError::InvalidMagic)
		| ContainerError::Unsupported { .. } => CheckFailureKind::Unsupported,
		ContainerError::Bootstrap(error)
		| ContainerError::ObjectMap {
			reason: ObjectMapError::Format(error),
			..
		}
		| ContainerError::Volume {
			reason: VolumeError::Format(VolumeFormatError::Format(error)),
			..
		} => format_failure(error),
		ContainerError::Volume {
			reason: VolumeError::Format(VolumeFormatError::Unsupported(_)),
			..
		} => CheckFailureKind::Unsupported,
		ContainerError::Limit(_) | ContainerError::Allocation => CheckFailureKind::ResourceLimit,
		ContainerError::CheckpointNotFound { .. } => CheckFailureKind::Selection,
		ContainerError::Read {
			source: ReadError::OutOfRange,
			..
		} => CheckFailureKind::Corrupt,
		ContainerError::Read { .. } => CheckFailureKind::Read,
		_ => CheckFailureKind::Corrupt,
	}
}

fn reader_failure<E: StdError + 'static>(error: &ReaderError<ReadError<E>>) -> CheckFailureKind {
	match error {
		ReaderError::Container(error) => container_failure(error),
		ReaderError::Corrupt(Corruption::PhysicalNode(error)) => format_failure(error),
		ReaderError::Unsupported(_) => CheckFailureKind::Unsupported,
		ReaderError::Allocation | ReaderError::Limit(_) => CheckFailureKind::ResourceLimit,
		ReaderError::Cancelled => CheckFailureKind::Cancelled,
		_ => CheckFailureKind::Corrupt,
	}
}

pub(super) const fn format_failure(error: &FormatError) -> CheckFailureKind {
	match error {
		FormatError::UnsupportedCheckpointFlags(_)
		| FormatError::UnsupportedObjectMapFlags(_)
		| FormatError::UnsupportedObjectMapSnapshot => CheckFailureKind::Unsupported,
		_ => CheckFailureKind::Corrupt,
	}
}
