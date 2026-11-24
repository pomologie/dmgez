//! Same-directory staging and publication (C7).

use std::{
	fs::File,
	io::{self, Read, Seek, SeekFrom, Write},
	path::Path,
};

use tempfile::NamedTempFile;
use thiserror::Error;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod replace;

/// Failure before a new output could be published.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublishError<E> {
	#[error("could not create staging file")]
	Create(#[source] io::Error),
	#[error("could not write staging file")]
	Write(#[source] E),
	#[error("could not sync staging file")]
	Sync(#[source] io::Error),
	#[error("could not seek to the start of the staged output")]
	Rewind(#[source] io::Error),
	#[error("staged output did not pass prepublication checks")]
	Validate(#[source] E),
	#[error("could not publish staged output without replacing an existing entry")]
	Commit(#[source] io::Error),
}

/// A read-only, seekable view of the synced staging data.
pub struct StagedReader<'file>(&'file mut File);

impl Read for StagedReader<'_> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		self.0.read(buffer)
	}
}

impl Seek for StagedReader<'_> {
	fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
		self.0.seek(position)
	}
}

/// Writes a new output in the destination directory and commits it after sync
/// and caller-supplied validation. The validator reads from offset zero.
/// The supplied directory must remain stable for the duration of publication.
/// Atomic visibility depends on the host filesystem's no-clobber primitive.
/// This function does not sync the parent directory.
///
/// # Errors
/// The existing destination survives failures, including a competing writer
/// creating it before commit. A write or validation error abandons the staging
/// file and returns the original error as its cause.
pub fn publish_new<T, E>(
	destination: &Path,
	write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
	validate: impl FnOnce(StagedReader<'_>, &T) -> Result<(), E>,
) -> Result<T, PublishError<E>> {
	let parent = destination
		.parent()
		.filter(|path| !path.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	let mut stage = NamedTempFile::new_in(parent).map_err(PublishError::Create)?;
	let report = prepare_stage(stage.as_file_mut(), write)?;
	validate(StagedReader(stage.as_file_mut()), &report).map_err(PublishError::Validate)?;
	stage
		.persist_noclobber(destination)
		.map_err(|failure| PublishError::Commit(failure.error))?;
	Ok(report)
}

fn prepare_stage<T, E>(
	stage: &mut File,
	write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
) -> Result<T, PublishError<E>> {
	let report = write(stage).map_err(PublishError::Write)?;
	stage.sync_all().map_err(PublishError::Sync)?;
	stage.rewind().map_err(PublishError::Rewind)?;
	Ok(report)
}
