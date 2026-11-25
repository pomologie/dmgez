//! Prepare and publish a dmgez project translated from appdmg settings.

use std::{
	io,
	path::{Path, absolute},
};

use dmgez_config::{AppDmgDiagnostic, AppDmgImport, AppDmgImportError, import_appdmg};
use dmgez_core::output::{PublishError, publish_new};
use thiserror::Error;

use crate::project_input::{self, InputError};

/// Appdmg settings could not be read or translated.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImportError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error("appdmg settings require a parent directory")]
	MissingDirectory,
	#[error(transparent)]
	Input(#[from] InputError),
	#[error(transparent)]
	Translation(#[from] AppDmgImportError),
}

/// A validated translation whose diagnostics can be presented before publication.
#[derive(Debug)]
pub struct ImportedProject(AppDmgImport);

impl ImportedProject {
	/// Loads bounded appdmg JSON and resolves relative sources against its directory.
	///
	/// # Errors
	/// Returns an error for unreadable or unsupported settings or invalid translated
	/// configuration.
	pub fn prepare(input: &Path, image_output: &str) -> Result<Self, ImportError> {
		let input = absolute(input)?;
		let directory = input.parent().ok_or(ImportError::MissingDirectory)?;
		let source = project_input::read(&input)?;
		Ok(Self(import_appdmg(&source, directory, image_output)?))
	}

	/// Diagnostics from features that need an explicit choice after translation.
	#[must_use]
	pub fn diagnostics(&self) -> &[AppDmgDiagnostic] {
		&self.0.diagnostics
	}

	/// Publishes the translated project as a new TOML file.
	///
	/// # Errors
	/// Returns a publication error for an existing output or a failed write or sync.
	pub fn publish(&self, output: &Path) -> Result<(), PublishError<io::Error>> {
		publish_new(
			output,
			|file| file.write_all(self.0.manifest.as_bytes()),
			|_, ()| Ok(()),
		)
	}
}
