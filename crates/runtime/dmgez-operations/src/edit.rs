//! Plan copy-on-write rebuilds (D-OPS-01).

#[cfg(feature = "image-edit")]
pub mod image_source;

use std::{
	collections::BTreeMap,
	fs, io,
	num::NonZeroUsize,
	path::{Path, PathBuf},
};

use dmgez_builder::preflight::{
	BuildCapabilities, ExecutionProfile, OutputIntent, PreflightError, PreflightOptions,
	ReleaseIntent, preflight,
};
use dmgez_config::{
	Project,
	project_edit::{ProjectDocument, ProjectEdit, ProjectEditError},
};
use thiserror::Error;

/// A typed overlay anchored to one exact source revision and project location.
pub struct ProjectRebuildRequest<'a> {
	/// Exact manifest text the edits were authored against.
	pub expected_source: &'a str,
	/// Manifest text from the caller's current snapshot.
	pub current_source: &'a str,
	/// The absolute configuration directory used for both revisions.
	pub config_dir: &'a Path,
	/// The definitions used to resolve the project in both revisions.
	pub defines: BTreeMap<String, String>,
	/// One atomic edit batch, including a new image destination.
	pub edits: &'a [ProjectEdit],
	/// Available functionality of the selected filesystem adapter.
	pub capabilities: BuildCapabilities,
	/// Number of available host threads for builder preflight.
	pub host_threads: NonZeroUsize,
}

/// A validated project and its preflight for rebuilding to a new image path.
///
/// The caller can inspect the edited TOML and hand the project, configuration
/// directory and options to the ordinary filesystem builder. The builder still
/// discovers sources, recalculates capacity and publishes without overwriting.
pub struct ProjectRebuild {
	document: ProjectDocument,
	config_dir: PathBuf,
	output_path: PathBuf,
	options: PreflightOptions,
	profile: ExecutionProfile,
}

impl ProjectRebuild {
	/// Project settings with the overlay applied.
	#[must_use]
	pub const fn project(&self) -> &Project {
		self.document.project()
	}

	/// Edited source text, including untouched formatting and comments.
	#[must_use]
	pub fn source(&self) -> &str {
		self.document.source()
	}

	/// Absolute configuration directory to pass to the filesystem builder.
	#[must_use]
	pub fn config_dir(&self) -> &Path {
		&self.config_dir
	}

	/// Absolute path intended for the rebuilt image.
	#[must_use]
	pub fn output_path(&self) -> &Path {
		&self.output_path
	}

	/// The unsigned, path-output options to pass to the filesystem builder.
	#[must_use]
	pub const fn options(&self) -> PreflightOptions {
		self.options
	}

	/// Preflight result, including whether configured release stages were deferred.
	#[must_use]
	pub const fn profile(&self) -> &ExecutionProfile {
		&self.profile
	}
}

/// A project could not be planned for copy-on-write rebuild.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProjectRebuildError {
	#[error("the current project differs from the expected edit revision")]
	SourceChanged,
	#[error(transparent)]
	Project(#[from] ProjectEditError),
	#[error("a copy-on-write rebuild requires overwrite to be disabled")]
	Overwrite,
	#[error(transparent)]
	Preflight(#[from] PreflightError),
	#[error("the configuration directory must be absolute")]
	RelativeConfigDir,
	#[error("the selected configuration directory is not a directory")]
	ConfigDirKind,
	#[error("could not inspect project or destination path {path:?}")]
	PathIo {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("the rebuilt image requires a new destination distinct from the original")]
	UnchangedOutput,
	#[error("rebuilt image destination has no filename")]
	OutputName,
	#[error("rebuilt image destination {path:?} already exists")]
	OutputExists { path: PathBuf },
}

/// Plans an unsigned rebuild from a project overlay without writing to disk.
///
/// All edits resolve against the same original project. The input manifest and
/// existing image stay untouched. Publication requires a subsequent builder run
/// with the returned project and options, in a stable destination directory.
///
/// # Errors
/// Rejects stale manifest text, conflicting or invalid edits, unsupported build
/// settings, overwrite requests, or a destination that is unchanged or occupied.
pub fn plan_project_rebuild(
	request: ProjectRebuildRequest<'_>,
) -> Result<ProjectRebuild, ProjectRebuildError> {
	if request.expected_source != request.current_source {
		return Err(ProjectRebuildError::SourceChanged);
	}
	let mut document = ProjectDocument::open_with_defines(request.current_source, request.defines)?;
	let original_output = document.project().image.output.clone();
	document.apply(request.edits)?;
	if document.project().build.overwrite {
		return Err(ProjectRebuildError::Overwrite);
	}
	let output = &document.project().image.output;
	if *output == original_output {
		return Err(ProjectRebuildError::UnchangedOutput);
	}
	let options = PreflightOptions {
		capabilities: request.capabilities,
		output: OutputIntent::Path,
		release: ReleaseIntent::Unsigned,
		host_threads: request.host_threads,
	};
	let profile = preflight(document.project(), options)?;
	if !request.config_dir.is_absolute() {
		return Err(ProjectRebuildError::RelativeConfigDir);
	}
	let config_dir =
		request
			.config_dir
			.canonicalize()
			.map_err(|source| ProjectRebuildError::PathIo {
				path: request.config_dir.to_path_buf(),
				source,
			})?;
	let config_metadata =
		fs::metadata(&config_dir).map_err(|source| ProjectRebuildError::PathIo {
			path: config_dir.clone(),
			source,
		})?;
	if !config_metadata.is_dir() {
		return Err(ProjectRebuildError::ConfigDirKind);
	}
	let output_path = output.resolve(&config_dir);
	let original_path = original_output.resolve(&config_dir);
	let candidate = canonical_output(&output_path)?;
	if canonical_output(&original_path).is_ok_and(|original| original == candidate) {
		return Err(ProjectRebuildError::UnchangedOutput);
	}
	match fs::symlink_metadata(&candidate) {
		Ok(_) => return Err(ProjectRebuildError::OutputExists { path: output_path }),
		Err(error) if error.kind() == io::ErrorKind::NotFound => {}
		Err(source) => {
			return Err(ProjectRebuildError::PathIo {
				path: output_path,
				source,
			});
		}
	}
	Ok(ProjectRebuild {
		document,
		config_dir,
		output_path,
		options,
		profile,
	})
}

fn canonical_output(path: &Path) -> Result<PathBuf, ProjectRebuildError> {
	let filename = path.file_name().ok_or(ProjectRebuildError::OutputName)?;
	let parent = path.parent().ok_or(ProjectRebuildError::OutputName)?;
	let parent = parent
		.canonicalize()
		.map_err(|source| ProjectRebuildError::PathIo {
			path: parent.to_path_buf(),
			source,
		})?;
	Ok(parent.join(filename))
}
