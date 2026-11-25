//! Host inspection of a project and its resolved installer sources.

use std::{io, path::Path};

use dmgez_config::{ConfigError, Project};
use dmgez_lint::installer::{self, Limits, Report, Suppression};
use thiserror::Error;

use crate::project_input::{self, InputError};

/// An installer project could not be admitted or linted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InstallerError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Input(#[from] InputError),
	#[error(transparent)]
	Config(#[from] ConfigError),
	#[error("project path has no parent directory")]
	MissingDirectory,
	#[error(transparent)]
	Lint(#[from] installer::Error),
}

/// Lints an installer project and its resolved source tree.
///
/// Relative sources resolve against the canonical manifest's parent.
///
/// # Errors
/// Returns an error for invalid projects, source or metadata failures, exhausted
/// limits, or cancellation.
pub fn lint_project<'input>(
	path: &Path,
	limits: Limits,
	suppressions: &[Suppression<'input>],
	cancelled: impl FnMut() -> bool,
) -> Result<Report<'input>, InstallerError> {
	let path = path.canonicalize()?;
	let project = Project::parse(&project_input::read(&path)?)?;
	let project_directory = path.parent().ok_or(InstallerError::MissingDirectory)?;
	Ok(installer::lint_installer(
		&project,
		project_directory,
		limits,
		suppressions,
		cancelled,
	)?)
}
