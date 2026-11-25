use std::{error::Error as StdError, num::NonZeroU64};

use apfs_format::{
	Error as FormatError,
	superblock::{BLOCK_SIZE_PREFIX_BYTES, BlockAddress, Superblock},
};
use dmgez_vfs::{Error as DiskError, LogicalDisk, Window};
use thiserror::Error;

use crate::{CheckFailureKind, verification::format_failure};

const BOOTSTRAP_BLOCK: BlockAddress = BlockAddress(0);
const BOOTSTRAP_OFFSET: u64 = 0;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProbeError<E: StdError + 'static = DiskError> {
	#[error("could not read the APFS block-zero superblock")]
	Disk(#[source] E),
	#[error("invalid APFS block-zero superblock")]
	Format(#[from] FormatError),
	#[error("could not allocate an APFS block-zero superblock buffer")]
	Allocation,
}

impl<E: StdError + 'static> ProbeError<E> {
	#[must_use]
	pub const fn kind(&self) -> CheckFailureKind {
		match self {
			Self::Disk(_) => CheckFailureKind::Read,
			Self::Format(error) => format_failure(error),
			Self::Allocation => CheckFailureKind::ResourceLimit,
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
	mut read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Option<NonZeroU64>, ProbeError<E>> {
	let mut prefix = [0; BLOCK_SIZE_PREFIX_BYTES];
	if length < prefix.len() as u64 {
		return Ok(None);
	}
	read(BOOTSTRAP_OFFSET, &mut prefix).map_err(ProbeError::Disk)?;
	let block_bytes = match Superblock::block_size_hint(&prefix) {
		Ok(bytes) => bytes,
		Err(FormatError::InvalidMagic) => return Ok(None),
		Err(error) => return Err(error.into()),
	};
	if u64::from(block_bytes) > length {
		return Err(FormatError::InvalidGeometry.into());
	}
	let mut block = Vec::new();
	block
		.try_reserve_exact(block_bytes as usize)
		.map_err(|_| ProbeError::Allocation)?;
	block.resize(block_bytes as usize, 0);
	read(BOOTSTRAP_OFFSET, &mut block).map_err(ProbeError::Disk)?;
	let geometry = Superblock::decode(&block, length)?.geometry;
	let range = geometry.byte_range(BOOTSTRAP_BLOCK, geometry.block_count())?;
	let bytes = NonZeroU64::new(range.end).ok_or(FormatError::InvalidGeometry)?;
	Ok(Some(bytes))
}
