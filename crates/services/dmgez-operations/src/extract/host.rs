//! Descriptor-relative publication of a selected fork into an absent file.
//!
//! The caller authorizes a stable local destination namespace before admission.
//! A directory descriptor pins the authorized identity. Path rechecks detect
//! observed replacement or movement before writing a chunk or publishing.
//! Rechecks and the following filesystem mutations are separate operations.
//! The caller must prevent uncooperative mutation of the authorized namespace
//! during extraction. The resulting file holds the selected fork's bytes.

mod directory;
mod stage;

use std::{error::Error as StdError, io, num::NonZeroUsize};

use dmgez_vfs::volume::{Fork, ViewError};
use thiserror::Error;

use super::HostName;
use stage::Stage;

pub use directory::{DestinationDirectory, HostError, StableDestination};

const MAX_CHUNK_BYTES: usize = 1 << 20;

/// Explicit bounds for one selected fork.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForkWriteLimits {
	pub max_fork_bytes: u64,
	/// Reusable read buffer size. A chunk can be at most 1 MiB.
	pub chunk_bytes: NonZeroUsize,
}

/// Failure to publish the admitted fork.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ForkWriteError<E: StdError + 'static> {
	#[error(transparent)]
	Host(#[from] HostError),
	#[error("selected fork length or chunk size exceeds the extraction limit")]
	Limit,
	#[error("could not allocate the extraction chunk buffer")]
	Reserve(#[from] std::collections::TryReserveError),
	#[error("extraction was cancelled")]
	Cancelled,
	#[error("could not read the selected fork exactly at offset {offset}")]
	Source {
		offset: u64,
		#[source]
		source: ViewError<E>,
	},
	#[error("could not write the staging file")]
	Write(#[source] io::Error),
	#[error("could not sync the staging file")]
	Sync(#[source] io::Error),
}

impl DestinationDirectory<'_> {
	/// Writes one selected VFS fork as a new, owner-only ordinary file.
	///
	/// Pass the session-bound fork lent by [dmgez_vfs::volume::VolumeView::with_fork].
	/// An empty fork creates an empty file. The declared name must be absent,
	/// including when the existing entry is a symbolic link. The file's data
	/// is synced before the no-clobber rename. Directory durability depends
	/// on platform and caller policy. On failure, the writer attempts cleanup
	/// when the staging name still identifies its owned file.
	///
	/// # Errors
	/// Reports cancellation, source or stage I/O failures, exceeded limits,
	/// namespace changes, destination conflicts, and unavailable no-clobber
	/// publication.
	pub fn publish_fork<E: StdError + 'static>(
		&self,
		name: &HostName,
		fork: &mut Fork<'_, E>,
		limits: ForkWriteLimits,
		mut cancelled: impl FnMut() -> bool,
	) -> Result<u64, ForkWriteError<E>> {
		let length = fork.len();
		let cap = limits.chunk_bytes.get();
		if length > limits.max_fork_bytes || i64::try_from(length).is_err() || cap > MAX_CHUNK_BYTES
		{
			return Err(ForkWriteError::Limit);
		}
		let chunk = usize::try_from(length.min(cap as u64)).map_err(|_| ForkWriteError::Limit)?;
		let mut buffer = Vec::new();
		buffer.try_reserve_exact(chunk)?;
		buffer.resize(chunk, 0);
		self.publish_with(name, length, &mut cancelled, |stage, cancelled| {
			stream_fork(self, fork, &mut buffer, cancelled, |bytes| {
				stage.verify_name()?;
				stage.append(bytes).map_err(ForkWriteError::Write)
			})
		})?;
		Ok(length)
	}

	fn publish_with<E: StdError + 'static>(
		&self,
		name: &HostName,
		length: u64,
		cancelled: &mut dyn FnMut() -> bool,
		write: impl FnOnce(&mut Stage<'_>, &mut dyn FnMut() -> bool) -> Result<(), ForkWriteError<E>>,
	) -> Result<(), ForkWriteError<E>> {
		check_cancel(cancelled)?;
		self.check_attachment()?;
		self.require_absent(name)?;
		let mut stage = Stage::create(self.descriptor(), self.root_path(), name)?;
		write(&mut stage, cancelled)?;
		stage.sync().map_err(ForkWriteError::Sync)?;
		check_cancel(cancelled)?;
		stage.verify_length(length)?;
		self.check_attachment()?;
		stage.publish(name)?;
		Ok(())
	}
}

fn stream_fork<E: StdError + 'static>(
	directory: &DestinationDirectory<'_>,
	fork: &mut Fork<'_, E>,
	buffer: &mut [u8],
	cancelled: &mut dyn FnMut() -> bool,
	mut write: impl FnMut(&[u8]) -> Result<(), ForkWriteError<E>>,
) -> Result<(), ForkWriteError<E>> {
	let length = fork.len();
	let mut offset = 0;
	while offset < length {
		check_cancel(cancelled)?;
		let size = (length - offset).min(buffer.len() as u64);
		let count = usize::try_from(size).map_err(|_| ForkWriteError::Limit)?;
		let chunk = buffer
			.get_mut(..count)
			.filter(|chunk| !chunk.is_empty())
			.ok_or(ForkWriteError::Limit)?;
		fork.read_exact_at(offset, chunk)
			.map_err(|source| ForkWriteError::Source { offset, source })?;
		check_cancel(cancelled)?;
		directory.check_attachment()?;
		write(chunk)?;
		offset += size;
	}
	check_cancel(cancelled)?;
	fork.read_exact_at(length, &mut [])
		.map_err(|source| ForkWriteError::Source {
			offset: length,
			source,
		})?;
	check_cancel(cancelled)?;
	Ok(())
}

fn check_cancel<E: StdError + 'static>(
	cancelled: &mut dyn FnMut() -> bool,
) -> Result<(), ForkWriteError<E>> {
	if cancelled() {
		return Err(ForkWriteError::Cancelled);
	}
	Ok(())
}

#[cfg(test)]
mod tests;
