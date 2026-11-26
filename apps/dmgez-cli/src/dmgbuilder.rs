//! Project planning and unsigned image construction from a `dmg.toml` file.

use std::{
	collections::BTreeMap,
	error::Error as StdError,
	fmt,
	path::{Path, PathBuf},
};

use dmgez_runtime::project_build::{BuildMode, BuildOutcome, BuildProject, ProjectRequest};

/// A loaded project with its source paths anchored to the manifest directory.
#[derive(Debug)]
pub struct Project(BuildProject);

/// A project admission, planning, or publication failure.
#[derive(Debug)]
pub struct Error(Box<dyn StdError + Send + Sync>);

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}

impl StdError for Error {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		Some(self.0.as_ref())
	}
}

/// The requested phase of an unsigned build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
	/// Resolve the image layout without writing an output file.
	Plan,
	/// Write and publish the image according to the project's output policy.
	Build,
}

/// Geometry and output information from a completed request.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
	/// The sealed plan, with its intended destination when writing to a file.
	Planned {
		output: Option<PathBuf>,
		logical_bytes: u64,
	},
	/// A completed unsigned build.
	Built {
		output: Option<PathBuf>,
		logical_bytes: u64,
		physical_bytes: u64,
	},
}

impl Project {
	/// Load a bounded TOML manifest from an absolute or working-directory-relative path.
	///
	/// # Errors
	///
	/// Returns an error if the working directory, manifest, or project configuration
	/// cannot be read or admitted.
	pub fn open(path: &Path) -> Result<Self, Error> {
		let working_directory = std::env::current_dir().map_err(|error| Error(Box::new(error)))?;
		BuildProject::load(ProjectRequest {
			path,
			working_directory: &working_directory,
			definitions: &BTreeMap::new(),
			overrides: Vec::new(),
			overwrite: false,
		})
		.map(Self)
		.map_err(|error| Error(Box::new(error)))
	}

	/// Plan or build the unsigned image. The published path follows the manifest's
	/// output and overwrite settings. Configured signing stages require the
	/// explicit CLI release workflow.
	///
	/// # Errors
	///
	/// Returns an error for unsupported project settings, changed inputs, or a
	/// failed build or publication.
	pub fn run(&self, mode: Mode) -> Result<Outcome, Error> {
		let mode = match mode {
			Mode::Plan => BuildMode::Plan,
			Mode::Build => BuildMode::Publish,
		};
		let outcome = self
			.0
			.run(mode, Default::default(), |_| {})
			.map_err(|error| Error(Box::new(error)))?;
		Ok(match outcome {
			BuildOutcome::Planned(plan) => Outcome::Planned {
				output: plan.output,
				logical_bytes: plan.logical_bytes,
			},
			BuildOutcome::Published { report, output, .. } => Outcome::Built {
				output,
				logical_bytes: report.image.logical_bytes,
				physical_bytes: report.image.physical_bytes,
			},
		})
	}
}
