//! Bounded native stapling and ticket validation.

use std::{ffi::OsStr, path::Path, process::ExitStatus};

use super::runner;

#[cfg(test)]
mod tests;

const STAPLE: &str = "staple";
const VALIDATE: &str = "validate";
const QUIET: &str = "-q";

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
	#[error("the native stapler is unavailable")]
	Unavailable,
	#[error("the native stapler requires an absolute image path")]
	RelativePath,
	#[error("could not run the native stapler")]
	Tool(#[source] runner::Error),
	#[error("the native stapler did not attach a ticket ({0})")]
	Staple(ExitStatus),
	#[error("the native stapler did not validate the ticket ({0})")]
	Validate(ExitStatus),
}

pub(super) struct Tool;

impl Tool {
	pub(super) fn new() -> Result<Self, Error> {
		let output = runner::find_stapler().map_err(|_| Error::Unavailable)?;
		if !output.status.success() || output.stdout.iter().all(u8::is_ascii_whitespace) {
			return Err(Error::Unavailable);
		}
		Ok(Self)
	}

	pub(super) fn staple(&self, path: &Path) -> Result<(), Error> {
		request(path, STAPLE, Error::Staple, runner::stapler)
	}

	pub(super) fn validate(&self, path: &Path) -> Result<(), Error> {
		request(path, VALIDATE, Error::Validate, runner::stapler)
	}
}

fn request(
	path: &Path,
	action: &str,
	exit_error: impl FnOnce(ExitStatus) -> Error,
	run: impl FnOnce(&[&OsStr]) -> Result<runner::Output, runner::Error>,
) -> Result<(), Error> {
	// An absolute operand starts with '/' regardless of its file name.
	if !path.is_absolute() {
		return Err(Error::RelativePath);
	}
	let output =
		run(&[OsStr::new(action), OsStr::new(QUIET), path.as_os_str()]).map_err(Error::Tool)?;
	if output.status.success() {
		Ok(())
	} else {
		Err(exit_error(output.status))
	}
}
