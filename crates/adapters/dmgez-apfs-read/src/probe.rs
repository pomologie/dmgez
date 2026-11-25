use std::{cell::RefCell, error::Error as StdError, num::NonZeroU64};

use apfs_container::{Error as ContainerError, probe_extent};
use apfs_core::{ByteOffset, ReadAt};
use apfs_format::Error as FormatError;
use dmgez_vfs::{Error as DiskError, LogicalDisk, Window};
use thiserror::Error;

use crate::{CheckFailureKind, verification::format_failure};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeError<E: StdError + 'static = DiskError> {
	#[error("could not read the APFS block-zero superblock")]
	Disk(#[source] E),
	#[error("invalid APFS block-zero superblock")]
	Format(#[from] FormatError),
	#[error("could not allocate an APFS block-zero superblock buffer")]
	Allocation,
	#[error("the APFS container probe failed")]
	Container(#[source] ContainerError<E>),
}

impl<E: StdError + 'static> ProbeError<E> {
	#[must_use]
	pub const fn kind(&self) -> CheckFailureKind {
		match self {
			Self::Disk(_) => CheckFailureKind::Read,
			Self::Format(error) => format_failure(error),
			Self::Allocation | Self::Container(ContainerError::Limit(_)) => {
				CheckFailureKind::ResourceLimit
			}
			Self::Container(_) => CheckFailureKind::Corrupt,
		}
	}
}

impl From<DiskError> for ProbeError {
	fn from(error: DiskError) -> Self {
		Self::Disk(error)
	}
}

/// Recognizes and bounds a partitionless APFS container by its checksummed
/// block-zero superblock. Checkpoint and volume admission follow separately.
///
/// # Errors
/// Fails on a source read error or a recognized header with invalid size,
/// checksum or geometry, and if the bounded scratch allocation fails.
pub fn probe_partitionless<S: LogicalDisk>(
	mut window: Window<'_, S>,
) -> Result<Option<NonZeroU64>, ProbeError> {
	probe(window.len(), |offset, output| {
		window.read_exact_at(offset, output)
	})
}

/// Recognizes a partitionless APFS container on any bounded read source.
///
/// # Errors
/// Fails on a source read error, malformed APFS block-zero metadata, or failure
/// to allocate the bounded scratch buffer.
pub fn probe<E: StdError + 'static>(
	length: u64,
	read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Option<NonZeroU64>, ProbeError<E>> {
	let source = ProbeSource {
		length,
		read: RefCell::new(read),
	};
	probe_extent(&source).map_err(|error| match error {
		ContainerError::Bootstrap(error) => ProbeError::Format(error),
		ContainerError::Read { source, .. } => ProbeError::Disk(source),
		ContainerError::Allocation => ProbeError::Allocation,
		error => ProbeError::Container(error),
	})
}

struct ProbeSource<F> {
	length: u64,
	read: RefCell<F>,
}

impl<F, E> ReadAt for ProbeSource<F>
where
	F: FnMut(u64, &mut [u8]) -> Result<(), E>,
{
	type Error = E;

	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		// The callback has no access to this source and the native probe reads sequentially.
		(self.read.borrow_mut())(offset.0, output)
	}
}
