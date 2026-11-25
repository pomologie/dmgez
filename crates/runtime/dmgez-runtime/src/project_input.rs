//! Bounded regular-file access for manifests and related text inputs.

use std::{
	fs::File,
	io::{self, ErrorKind, Read},
	path::Path,
};

use dmgez_config::{ConfigError, MAX_MANIFEST_BYTES};
use thiserror::Error;

const REGULAR_INPUT_REQUIRED: &str = "configuration input must be a regular file";

/// A manifest that could not be read within its input bounds.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InputError {
	#[error(transparent)]
	Limit(ConfigError),
	#[error(transparent)]
	Io(#[from] io::Error),
}

/// Read a UTF-8 manifest within the configuration parser's input bound.
///
/// # Errors
/// Returns an I/O error for invalid inputs or the manifest size-limit error.
pub fn read(path: &Path) -> Result<String, InputError> {
	read_limited(path, MAX_MANIFEST_BYTES).map_err(|error| {
		if error.kind() == ErrorKind::FileTooLarge {
			InputError::Limit(ConfigError::TooLarge)
		} else {
			error.into()
		}
	})
}

/// Read a UTF-8 configuration input without accepting more than `max_bytes`.
///
/// # Errors
/// Returns an I/O error when the path is unreadable, non-regular, too large, or
/// contains invalid UTF-8.
pub fn read_limited(path: &Path, max_bytes: usize) -> io::Result<String> {
	let too_large = || {
		io::Error::new(
			ErrorKind::FileTooLarge,
			format!("configuration input exceeds the {max_bytes}-byte input limit"),
		)
	};
	if !path.metadata()?.is_file() {
		return Err(io::Error::new(
			ErrorKind::InvalidInput,
			REGULAR_INPUT_REQUIRED,
		));
	}
	let file = File::open(path)?;
	let metadata = file.metadata()?;
	if !metadata.is_file() {
		return Err(io::Error::new(
			ErrorKind::InvalidInput,
			REGULAR_INPUT_REQUIRED,
		));
	}
	let limit = max_bytes as u64;
	if metadata.len() > limit {
		return Err(too_large());
	}
	let mut source = String::new();
	file.take(limit.saturating_add(1))
		.read_to_string(&mut source)?;
	if source.len() > max_bytes {
		return Err(too_large());
	}
	Ok(source)
}
