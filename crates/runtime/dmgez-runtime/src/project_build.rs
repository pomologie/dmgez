//! Admit and build HFS+ images from project manifests.

use std::{
	collections::BTreeMap,
	io,
	path::{Path, PathBuf},
	thread,
};

use dmgez_builder::{
	execution::BuildReport,
	partition::Layout,
	planning::PreparedBuild,
	preflight::{
		Availability, FilesystemProfile, FinderPlan, OutputIntent, OutputPlan, PreflightOptions,
		ReleaseIntent, ReleasePlan,
	},
	progress::BuildProgress,
};
use dmgez_config::{ConfigError, ConfigOverride, ImageFormat, OverrideError, Project};
use dmgez_hfsplus::{self as hfsplus, HfsImage, MetadataLoss, PreparedHfsBuild};
use dmgez_udif::writer::Cancellation;
use thiserror::Error;

use crate::project_input::{self, InputError};

const OVERWRITE_ASSIGNMENT: &str = "build.overwrite=true";

/// Manifest selection and explicit overrides for the requested build.
pub struct ProjectRequest<'source> {
	/// Absolute path or a path relative to the caller's working directory.
	pub path: &'source Path,
	/// Absolute directory used to resolve the manifest and explicit override paths.
	pub working_directory: &'source Path,
	/// Literal definition values that take precedence over manifest definitions.
	pub definitions: &'source BTreeMap<String, String>,
	/// Typed overrides in order of application.
	pub overrides: Vec<ConfigOverride>,
	/// Whether to explicitly request replacement independently of the overrides.
	pub overwrite: bool,
}

/// An admitted manifest with the directory used for project-relative paths.
#[derive(Debug)]
pub struct BuildProject {
	project: Project,
	directory: PathBuf,
}

/// Errors during manifest admission, preparation, sealing, or publication.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BuildError {
	#[error("working directory must be absolute")]
	RelativeWorkingDirectory,
	#[error("project path has no parent directory")]
	MissingProjectDirectory,
	#[error("build.overwrite was requested more than once")]
	DuplicateOverwrite,
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Input(#[from] InputError),
	#[error(transparent)]
	Config(#[from] ConfigError),
	#[error(transparent)]
	Override(#[from] OverrideError),
	#[error(transparent)]
	Hfsplus(#[from] hfsplus::Error),
}

/// The requested operation on a sealed unsigned image.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BuildMode {
	/// Resolve exact geometry and report the plan without writing the image.
	Plan,
	/// Stage and publish to the destination admitted by preflight.
	Publish,
}

/// Observations delivered synchronously throughout a build.
#[derive(Debug)]
pub enum BuildEvent<'build> {
	/// Source preparation or image publication progress.
	Progress(BuildProgress),
	/// Metadata omissions identified at sealing, before any publication attempt.
	MetadataLosses(&'build [MetadataLoss<'build>]),
}

/// The resulting build plan or publication evidence.
#[derive(Debug)]
pub enum BuildOutcome {
	Planned(PlanSummary),
	Published {
		report: BuildReport,
		metadata_losses: usize,
	},
}

/// Resolved policy, source counts, and geometry of a sealed image plan.
#[derive(Debug)]
#[non_exhaustive]
pub struct PlanSummary {
	pub format: ImageFormat,
	pub filesystem: FilesystemProfile,
	pub partition: Layout,
	pub finder: FinderPlan,
	pub output: Option<PathBuf>,
	pub publication: OutputPlan,
	pub release: ReleasePlan,
	pub compression_cache: bool,
	pub logical_bytes: u64,
	/// Exact only for formats whose physical size is determined during sealing.
	pub physical_bytes: Option<u64>,
	pub resolved_entries: usize,
	pub source_payload_bytes: u64,
	pub finder_assets: usize,
	pub metadata_losses: usize,
}

impl BuildProject {
	/// Read a bounded manifest, apply typed overrides, and validate the project.
	///
	/// # Errors
	/// Returns path, manifest input, override, or configuration errors.
	pub fn load(request: ProjectRequest<'_>) -> Result<Self, BuildError> {
		let ProjectRequest {
			path,
			working_directory,
			definitions,
			mut overrides,
			overwrite,
		} = request;
		if !working_directory.is_absolute() {
			return Err(BuildError::RelativeWorkingDirectory);
		}
		let path = working_directory.join(path).canonicalize()?;
		let directory = path.parent().ok_or(BuildError::MissingProjectDirectory)?;
		let input = project_input::read(&path)?;
		if overwrite {
			let overwrite = ConfigOverride::parse(OVERWRITE_ASSIGNMENT)?;
			if overrides
				.iter()
				.any(|assignment| assignment.key() == overwrite.key())
			{
				return Err(BuildError::DuplicateOverwrite);
			}
			overrides.push(overwrite);
		}
		let project =
			Project::parse_with_overrides(&input, definitions, &overrides, working_directory)?;
		Ok(Self {
			project,
			directory: directory.to_owned(),
		})
	}

	/// The effective project, for callers admitting an explicit release workflow.
	#[must_use]
	pub const fn project(&self) -> &Project {
		&self.project
	}

	/// Plan or publish the unsigned image and deliver observations to the caller.
	///
	/// # Errors
	/// Returns host resource, preflight, source, sealing, or publication errors.
	pub fn run(
		&self,
		mode: BuildMode,
		cancellation: Cancellation,
		observe: impl Fn(BuildEvent<'_>),
	) -> Result<BuildOutcome, BuildError> {
		let progress = |event| observe(BuildEvent::Progress(event));
		let prepared = self.prepare(ReleaseIntent::Unsigned, cancellation, progress)?;
		let image = prepared.seal()?;
		observe(BuildEvent::MetadataLosses(image.metadata_losses()));
		match mode {
			BuildMode::Plan => Ok(BuildOutcome::Planned(PlanSummary::from_sealed(
				prepared.prepared(),
				&image,
			))),
			BuildMode::Publish => {
				let published = image.publish_with_progress(None, progress)?;
				let metadata_losses = metadata_loss_count(&published.metadata_losses);
				Ok(BuildOutcome::Published {
					report: published.report,
					metadata_losses,
				})
			}
		}
	}

	/// Prepare sources for a release that the caller has independently admitted.
	/// The caller supplies verification and all signing, notarization, and
	/// publication steps for the release artifact.
	///
	/// # Errors
	/// Returns host resource, preflight, or source preparation errors.
	#[cfg(feature = "project-release")]
	pub(super) fn prepare_release(
		&self,
		cancellation: Cancellation,
		progress: impl Fn(BuildProgress),
	) -> Result<PreparedHfsBuild<'_>, BuildError> {
		self.prepare(ReleaseIntent::Release, cancellation, progress)
	}

	fn prepare(
		&self,
		release: ReleaseIntent,
		cancellation: Cancellation,
		progress: impl Fn(BuildProgress),
	) -> Result<PreparedHfsBuild<'_>, BuildError> {
		let mut capabilities = hfsplus::capabilities();
		if release == ReleaseIntent::Release {
			capabilities.release = Availability::Available;
			capabilities.verify = Availability::Available;
		}
		Ok(hfsplus::prepare_with_progress(
			&self.project,
			&self.directory,
			PreflightOptions {
				capabilities,
				output: OutputIntent::Path,
				release,
				host_threads: thread::available_parallelism()?,
			},
			cancellation,
			progress,
		)?)
	}
}

impl PlanSummary {
	/// Summarize frozen geometry for either an unsigned image or a staged release.
	#[must_use]
	pub(super) fn from_sealed(prepared: &PreparedBuild, image: &HfsImage<'_>) -> Self {
		let profile = prepared.profile();
		let manifest = prepared.manifest();
		let logical_bytes = image.logical_bytes();
		#[cfg(feature = "finder-write")]
		let finder_assets = prepared.finder_asset_count();
		#[cfg(not(feature = "finder-write"))]
		let finder_assets = 0;
		Self {
			format: profile.format(),
			filesystem: profile.filesystem(),
			partition: profile.partition(),
			finder: profile.finder(),
			output: prepared.output_path().map(Path::to_owned),
			publication: profile.output(),
			release: profile.release(),
			compression_cache: profile.cache().is_some(),
			logical_bytes,
			physical_bytes: (profile.format() == ImageFormat::Udrw).then_some(logical_bytes),
			resolved_entries: manifest.entries().len(),
			source_payload_bytes: manifest.regular_payload_bytes(),
			finder_assets,
			metadata_losses: metadata_loss_count(image.metadata_losses()),
		}
	}
}

pub(crate) fn metadata_loss_count(entries: &[MetadataLoss<'_>]) -> usize {
	entries.iter().map(|entry| entry.details.len()).sum()
}
