//! Execute an explicitly authorized staged project release.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use dmgez_builder::{
	execution::StagedRelease, planning::PreparedBuild, preflight::ReleaseIntent,
	progress::BuildProgress,
};
use dmgez_config::Project;
use dmgez_hfsplus::HfsImage;
use dmgez_signing::release_plan::{
	NotaryOutcome, ReleaseAction, ReleaseApplications, ReleaseError, ReleaseReceipt, ReleaseReport,
	ReleaseRun, ReleaseStage, StageFailure, StageStatus,
};
use dmgez_udif::writer::Cancellation;
use thiserror::Error;

use crate::project_build::{BuildError, BuildEvent, BuildMode, BuildProject, PlanSummary};

#[cfg(all(
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
))]
mod applications;
mod artifact;
mod native;
mod settings;

pub use settings::{AdmissionError, Stage as AdmissionStage};
use settings::{ReleaseCapabilities, ReleaseSettings};

type StageCause = Box<dyn std::error::Error + Send + Sync>;
type StageResult<T = ()> = Result<T, StageCause>;

const ARTIFACT_DIRECTORY_PREFIX: &str = ".dmgez-release-";
const SIGNED_IMAGE: &str = "signed.dmg";
const STAPLED_IMAGE: &str = "stapled.dmg";
const STAGE_MISSING: &str = "release stage is missing its input image";
const APPLICATION_STAGE_MISSING: &str =
	"release stage is missing the prepared application inventory";
const APPLICATION_VERIFICATION_AVAILABLE: bool = cfg!(all(
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
));

/// Options supplied by a caller that explicitly requests release stages.
pub struct ReleaseOptions<'input> {
	/// Plan the configured release or perform its stages and attempt publication.
	pub mode: BuildMode,
	/// An optional override of the configured disk-signing identifier.
	pub identifier: Option<&'input str>,
	/// Destination for the notarization record, if a submission is requested.
	pub notarization_record: Option<&'input Path>,
	/// Absolute directory used to anchor an explicit relative record path.
	pub working_directory: &'input Path,
}

/// The plan or execution evidence of a release request.
#[derive(Debug)]
pub enum ReleaseOutcome {
	Planned(PlanSummary),
	Executed(Box<ReleaseExecution>),
}

/// Evidence from a started release, including failed or suspended stages.
#[derive(Debug)]
#[non_exhaustive]
pub struct ReleaseExecution {
	pub report: ReleaseReport,
	pub published_path: Option<PathBuf>,
	pub retained_signed_image: Option<PathBuf>,
	pub notarization_record: Option<PathBuf>,
	/// The terminal result alongside evidence for the stages already attempted.
	pub result: Result<(), ReleaseWorkflowError>,
}

/// A release could not be admitted, completed, or published.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReleaseWorkflowError {
	#[error("release publication requires a new destination")]
	Overwrite,
	#[error("release settings are missing")]
	MissingSettings,
	#[error("configured notarization requires a record destination")]
	MissingRecord,
	#[error("a notarization record requires configured notarization")]
	UnexpectedRecord,
	#[error("missing release destination")]
	MissingOutput,
	#[error("release destination and notarization record must be different paths")]
	SharedDestination,
	#[error("release stopped before publication")]
	PublicationIncomplete,
	#[error(transparent)]
	Admission(#[from] AdmissionError),
	#[error(transparent)]
	Build(#[from] BuildError),
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Plan(#[from] ReleaseError),
	#[error(transparent)]
	Stage(StageCause),
}

/// Plan or execute a release using the configured signing and notarization policies.
///
/// # Errors
/// Returns errors that prevent a plan or release from starting. Once execution
/// begins, its terminal result accompanies the available stage evidence.
pub fn run(
	project: &BuildProject,
	options: ReleaseOptions<'_>,
	cancellation: Cancellation,
	observe: impl Fn(BuildEvent<'_>),
) -> Result<ReleaseOutcome, ReleaseWorkflowError> {
	let request = AdmittedRelease::admit(project.project(), &options)?;
	let progress = |event| observe(BuildEvent::Progress(event));
	let prepared = project.prepare_release(cancellation, progress)?;
	let image = prepared.seal().map_err(BuildError::from)?;
	observe(BuildEvent::MetadataLosses(image.metadata_losses()));
	match options.mode {
		BuildMode::Plan => Ok(ReleaseOutcome::Planned(PlanSummary::from_sealed(
			prepared.prepared(),
			&image,
		))),
		BuildMode::Publish => Ok(ReleaseOutcome::Executed(Box::new(request.execute(
			image,
			prepared.prepared(),
			progress,
		)?))),
	}
}

struct AdmittedRelease<'input> {
	settings: ReleaseSettings<'input>,
	record: Option<PathBuf>,
}

impl<'input> AdmittedRelease<'input> {
	fn admit(
		project: &'input Project,
		options: &ReleaseOptions<'input>,
	) -> Result<Self, ReleaseWorkflowError> {
		if !options.working_directory.is_absolute() {
			return Err(BuildError::RelativeWorkingDirectory.into());
		}
		if project.build.overwrite {
			return Err(ReleaseWorkflowError::Overwrite);
		}
		let native_signing = cfg!(all(target_os = "macos", feature = "signing-native"));
		let settings = ReleaseSettings::admit(
			project,
			ReleaseIntent::Release,
			options.identifier,
			ReleaseCapabilities {
				application_verification: APPLICATION_VERIFICATION_AVAILABLE,
				disk_signing: native_signing,
				ad_hoc_disk_signing: cfg!(all(
					feature = "signing-rust",
					any(target_os = "linux", target_os = "macos", windows),
				)),
				notarization: cfg!(all(target_os = "macos", feature = "notarization-native")),
				native_assessment: native_signing,
			},
		)?
		.ok_or(ReleaseWorkflowError::MissingSettings)?;
		let record = match (settings.notarization.is_some(), options.notarization_record) {
			(true, Some(record)) => Some(options.working_directory.join(record)),
			(true, None) => return Err(ReleaseWorkflowError::MissingRecord),
			(false, Some(_)) => return Err(ReleaseWorkflowError::UnexpectedRecord),
			(false, None) => None,
		};
		Ok(Self { settings, record })
	}

	fn execute(
		self,
		image: HfsImage<'_>,
		prepared: &PreparedBuild,
		progress: impl Fn(BuildProgress),
	) -> Result<ReleaseExecution, ReleaseWorkflowError> {
		let output = fresh_destination(
			prepared
				.output_path()
				.ok_or(ReleaseWorkflowError::MissingOutput)?,
		)?;
		let record = self.record.as_deref().map(fresh_destination).transpose()?;
		if record.as_ref() == Some(&output) {
			return Err(ReleaseWorkflowError::SharedDestination);
		}
		let mut run = self.settings.plan.start()?;
		let mut artifacts = Artifacts::default();
		let inputs = Inputs {
			prepared,
			settings: &self.settings,
			record: record.as_deref(),
			output: &output,
		};
		let result = drive(&mut run, &mut artifacts, image, &inputs, progress);
		let published =
			run.report().steps.iter().any(|step| {
				step.stage == ReleaseStage::Publish && step.status == StageStatus::Passed
			});
		if published {
			artifacts.restore_cleanup();
			artifacts.retained_path = None;
		}
		Ok(ReleaseExecution {
			report: run.into_report(),
			published_path: published.then_some(output),
			retained_signed_image: artifacts.retained_path,
			notarization_record: record,
			result: result.map_err(ReleaseWorkflowError::Stage).and(
				published
					.then_some(())
					.ok_or(ReleaseWorkflowError::PublicationIncomplete),
			),
		})
	}
}

#[derive(Default)]
struct Artifacts<'build> {
	unsigned: Option<StagedRelease<'build>>,
	current: Option<PathBuf>,
	private: Option<tempfile::TempDir>,
	retained_path: Option<PathBuf>,
	applications: Option<Vec<dmgez_config::VirtualPath>>,
}

impl Artifacts<'_> {
	fn path(&self) -> StageResult<&Path> {
		self.current.as_deref().ok_or_else(|| STAGE_MISSING.into())
	}

	fn validate_sources(&self) -> StageResult {
		Ok(self
			.unsigned
			.as_ref()
			.ok_or(STAGE_MISSING)?
			.validate_sources()?)
	}

	fn private_output(&mut self, output: &Path, name: &str) -> io::Result<PathBuf> {
		if self.private.is_none() {
			let parent = output.parent().ok_or(io::ErrorKind::InvalidInput)?;
			let mut builder = tempfile::Builder::new();
			builder.prefix(ARTIFACT_DIRECTORY_PREFIX);
			#[cfg(unix)]
			{
				use std::os::unix::fs::PermissionsExt;
				const OWNER_DIRECTORY_MODE: u32 = 0o700;
				builder.permissions(fs::Permissions::from_mode(OWNER_DIRECTORY_MODE));
			}
			self.private = Some(builder.tempdir_in(parent)?);
		}
		self.private
			.as_ref()
			.map(|directory| directory.path().join(name))
			.ok_or_else(|| io::ErrorKind::NotFound.into())
	}

	fn retain_for_notarization(&mut self) -> StageResult {
		let path = self.path()?.to_owned();
		self.private
			.as_mut()
			.ok_or(STAGE_MISSING)?
			.disable_cleanup(true);
		self.retained_path = Some(path);
		Ok(())
	}

	fn restore_cleanup(&mut self) {
		if let Some(directory) = &mut self.private {
			directory.disable_cleanup(false);
		}
	}
}

struct Inputs<'run, 'project> {
	prepared: &'run PreparedBuild,
	settings: &'run ReleaseSettings<'project>,
	record: Option<&'run Path>,
	output: &'run Path,
}

fn drive<'build>(
	run: &mut ReleaseRun,
	artifacts: &mut Artifacts<'build>,
	image: HfsImage<'build>,
	inputs: &Inputs<'_, '_>,
	progress: impl Fn(BuildProgress),
) -> StageResult {
	let mut image = Some(image);
	loop {
		let observed = artifacts
			.current
			.as_deref()
			.map(artifact::observe)
			.transpose()?;
		let Some(action) = run.begin(observed.as_ref())? else {
			return Ok(());
		};
		let receipt = match execute(action, artifacts, &mut image, inputs, &progress) {
			Ok(receipt) => receipt,
			Err(error) => {
				let failure = error.downcast_ref::<native::NativeReleaseError>().map_or(
					StageFailure::Failed,
					native::NativeReleaseError::stage_failure,
				);
				run.fail(failure)?;
				return Err(error);
			}
		};
		let published = matches!(&receipt, ReleaseReceipt::Published(_));
		run.complete(receipt)?;
		if published {
			return Ok(());
		}
	}
}

fn execute<'build>(
	action: ReleaseAction<'_>,
	artifacts: &mut Artifacts<'build>,
	image: &mut Option<HfsImage<'build>>,
	inputs: &Inputs<'_, '_>,
	progress: &impl Fn(BuildProgress),
) -> StageResult<ReleaseReceipt> {
	let Inputs {
		prepared,
		settings,
		record,
		output,
	} = *inputs;
	match action {
		ReleaseAction::PrepareApplications(ReleaseApplications::Verify(backend)) => {
			artifacts.applications = Some(native::prepare_applications(prepared, backend)?);
			Ok(ReleaseReceipt::ApplicationsPrepared(
				ReleaseApplications::Verify(backend),
			))
		}
		ReleaseAction::BuildUnsigned => {
			let mut stage = image
				.take()
				.ok_or(STAGE_MISSING)?
				.stage_release(&artifact::verify_stage, progress)?;
			let bytes = stage.report().physical_bytes;
			let identity = artifact::observe_stage(stage.reader()?, bytes)?;
			artifacts.current = Some(stage.path().to_owned());
			artifacts.unsigned = Some(stage);
			Ok(ReleaseReceipt::Built(identity))
		}
		ReleaseAction::VerifyUnsignedImage(identity) => {
			if artifact::verify(artifacts.path()?)? != *identity {
				return Err(ReleaseError::ArtifactChanged.into());
			}
			Ok(ReleaseReceipt::UnsignedVerified(identity.clone()))
		}
		ReleaseAction::VerifyApplications { backend, input } => {
			artifacts.validate_sources()?;
			let applications = artifacts
				.applications
				.as_deref()
				.ok_or(APPLICATION_STAGE_MISSING)?;
			let count = native::verify_applications(
				artifacts.path()?,
				input,
				prepared,
				applications,
				backend,
			)?;
			artifacts.validate_sources()?;
			Ok(ReleaseReceipt::ApplicationsVerified {
				backend,
				input: input.clone(),
				count,
			})
		}
		ReleaseAction::SignImage { backend, input } => {
			artifacts.validate_sources()?;
			let settings = settings
				.disk_signing
				.as_ref()
				.ok_or("missing disk-signing settings")?;
			let signed = artifacts.private_output(output, SIGNED_IMAGE)?;
			let report = native::sign_image(artifacts.path()?, &signed, settings)?;
			let observed = artifact::observe(&signed)?;
			if report.backend != backend
				|| report.unsigned_sha256 != input.sha256()
				|| report.signed_sha256 != observed.sha256()
			{
				return Err("release signing evidence does not match the staged bytes".into());
			}
			artifacts.current = Some(signed);
			Ok(ReleaseReceipt::Signed {
				backend,
				input: input.clone(),
				output: observed,
			})
		}
		ReleaseAction::Submit { .. } => {
			artifacts.validate_sources()?;
			let settings = settings
				.notarization
				.as_ref()
				.ok_or("missing notarization settings")?;
			artifacts.retain_for_notarization()?;
			let evidence = native::submit(
				artifacts.path()?,
				record.ok_or("missing notarization record")?,
				settings,
			)?;
			if evidence.outcome == NotaryOutcome::RejectedWithVerifiedDigest {
				artifacts.restore_cleanup();
				artifacts.retained_path = None;
			}
			Ok(ReleaseReceipt::Notarized(evidence))
		}
		ReleaseAction::Staple { backend, input, id } => {
			let stapled = artifacts.private_output(output, STAPLED_IMAGE)?;
			let report = native::staple(
				artifacts.path()?,
				record.ok_or("missing notarization record")?,
				&stapled,
			)?;
			let observed = artifact::observe(&stapled)?;
			if report.backend != backend
				|| report.submission_id != id
				|| report.input_sha256 != input.sha256()
				|| report.output_sha256 != observed.sha256()
			{
				return Err("release ticket evidence does not match the staged bytes".into());
			}
			artifacts.current = Some(stapled);
			Ok(ReleaseReceipt::Stapled {
				backend,
				submission_id: id.to_owned(),
				input: input.clone(),
				output: observed,
			})
		}
		ReleaseAction::VerifyImage(identity) => {
			let path = artifacts.path()?;
			if artifact::verify(path)? != *identity {
				return Err(ReleaseError::ArtifactChanged.into());
			}
			if let Some(settings) = &settings.disk_signing {
				native::verify_image(path, settings)?;
			}
			Ok(ReleaseReceipt::Verified(identity.clone()))
		}
		ReleaseAction::AssessNativePolicy(identity) => {
			native::assess(artifacts.path()?)?;
			Ok(ReleaseReceipt::Assessed(identity.clone()))
		}
		ReleaseAction::Publish(identity) => {
			artifacts.validate_sources()?;
			artifact::publish(artifacts.path()?, identity, output)?;
			Ok(ReleaseReceipt::Published(identity.clone()))
		}
		ReleaseAction::PrepareApplications(_) | ReleaseAction::Poll { .. } => {
			Err("release stage is unavailable in this build".into())
		}
	}
}

fn fresh_destination(path: &Path) -> io::Result<PathBuf> {
	let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
	let parent = path
		.parent()
		.ok_or(io::ErrorKind::InvalidInput)?
		.canonicalize()?;
	let path = parent.join(name);
	match fs::symlink_metadata(&path) {
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path),
		Ok(_) => Err(io::Error::new(
			io::ErrorKind::AlreadyExists,
			format!("output already exists at {path:?}"),
		)),
		Err(error) => Err(error),
	}
}
