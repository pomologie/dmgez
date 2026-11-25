//! Private staging and no-clobber publication of a raw disk as a UDIF image.

use std::{fs::File, io, path::Path};

use dmgez_core::{
	output::{PublishError, publish_new},
	source::{CapturedFile, SourceError},
};
use dmgez_udif::{
	Error as ImageError,
	writer::{self, WriteOptions, WriteReport},
};
use thiserror::Error;

/// Wrapping failed before the complete image could be published.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Open(#[from] io::Error),
	#[error(transparent)]
	Source(#[from] SourceError),
	#[error(transparent)]
	Publish(#[from] PublishError<ImageError>),
}

/// Wraps a regular raw disk file into a new UDIF image at the given path.
///
/// The source is held and revalidated before the completed stage is published.
/// An existing destination survives all failures.
///
/// # Errors
/// Returns input admission, encoding, stability, staging or publication errors.
pub fn file(
	input: &Path,
	destination: &Path,
	options: &WriteOptions,
) -> Result<WriteReport, Error> {
	let input = CapturedFile::new(File::open(input)?)?;
	Ok(publish_new(
		destination,
		|stage| writer::write_image(input.reader(), input.len(), stage, options),
		|_, _| {
			input.validate().map_err(|error| match error {
				SourceError::Changed => ImageError::SourceChanged,
				SourceError::Io(error) => error.into(),
				error => io::Error::other(error).into(),
			})
		},
	)?)
}
