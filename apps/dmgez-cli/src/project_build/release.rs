use std::{
	fs, io,
	io::Write,
	path::{Path, PathBuf},
};

use dmgez_builder::{execution::StagedRelease, preflight::ReleaseIntent, progress::BuildProgress};
use dmgez_config::Project;
use dmgez_hfsplus::HfsImage;
use dmgez_signing::release_plan::{
	NotaryOutcome, ReleaseAction, ReleaseError, ReleaseReceipt, ReleaseReport, ReleaseRun,
	ReleaseStage, StageFailure, StageStatus,
};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

use super::{
	Arguments, release_artifact, release_native,
	release_settings::{ReleaseCapabilities, ReleaseSettings},
};

const ARTIFACT_DIRECTORY_PREFIX: &str = ".dmgez-release-";
const SIGNED_IMAGE: &str = "signed.dmg";
const STAPLED_IMAGE: &str = "stapled.dmg";
const STAGE_MISSING: &str = "release stage is missing its input image";

pub(super) struct ReleaseRequest<'input> {
	settings: ReleaseSettings<'input>,
	record: Option<PathBuf>,
}

impl<'input> ReleaseRequest<'input> {
	pub(super) fn admit(
		project: &'input Project,
		arguments: &'input Arguments,
		working_directory: &Path,
	) -> CliResult<Option<Self>> {
		if !arguments.release {
			return Ok(None);
		}
		if project.build.overwrite {
			return Err("release publication requires a new destination".into());
		}
		let native_signing = cfg!(all(target_os = "macos", feature = "signing-native"));
		let settings = ReleaseSettings::admit(
			project,
			ReleaseIntent::Release,
			arguments.identifier.as_deref(),
			ReleaseCapabilities {
				application_verification: false,
				disk_signing: native_signing,
				notarization: cfg!(all(target_os = "macos", feature = "notarization-native")),
				native_assessment: native_signing,
			},
		)?
		.ok_or("release settings are missing")?;
		let record = match (settings.notarization.is_some(), &arguments.notary_record) {
			(true, Some(record)) => Some(working_directory.join(record)),
			(true, None) => return Err("configured notarization requires --notary-record".into()),
			(false, Some(_)) => {
				return Err("--notary-record requires configured notarization".into());
			}
			(false, None) => None,
		};
		Ok(Some(Self { settings, record }))
	}

	pub(super) fn run(
		self,
		image: HfsImage<'_>,
		output: &Path,
		json: bool,
		progress: impl Fn(BuildProgress),
	) -> CliResult {
		let output = fresh_destination(output)?;
		let record = self.record.as_deref().map(fresh_destination).transpose()?;
		if record.as_ref() == Some(&output) {
			return Err(
				"release destination and notarization record must be different paths".into(),
			);
		}
		let mut run = self.settings.plan.start()?;
		let mut artifacts = Artifacts::default();
		let result = drive(
			&mut run,
			&mut artifacts,
			image,
			&self.settings,
			record.as_deref(),
			&output,
			progress,
		);
		let published =
			run.report().steps.iter().any(|step| {
				step.stage == ReleaseStage::Publish && step.status == StageStatus::Passed
			});
		let retained = if published {
			artifacts.restore_cleanup();
			None
		} else {
			artifacts.retained_path.as_deref()
		};
		let report = CommandReport {
			stages: run.report(),
			published_path: published.then(|| output.to_string_lossy()),
			retained_signed_image: retained.map(Path::to_string_lossy),
			notarization_record: record.as_deref().map(Path::to_string_lossy),
		};
		write_report(json, &report, |writer| {
			for step in &report.stages.steps {
				writeln!(writer, "{:?} = {:?}", step.stage, step.status)?;
			}
			if let Some(path) = &report.published_path {
				writeln!(writer, "published = {}", EscapedText(path))?;
			}
			if let Some(path) = &report.retained_signed_image {
				writeln!(writer, "retained signed image = {}", EscapedText(path))?;
			}
			if let Some(path) = &report.notarization_record {
				writeln!(writer, "notarization record = {}", EscapedText(path))?;
			}
			Ok(())
		})?;
		result?;
		if !published {
			return Err("release stopped before publication".into());
		}
		Ok(())
	}
}

#[derive(Serialize)]
struct CommandReport<'run> {
	#[serde(flatten)]
	stages: &'run ReleaseReport,
	published_path: Option<std::borrow::Cow<'run, str>>,
	retained_signed_image: Option<std::borrow::Cow<'run, str>>,
	notarization_record: Option<std::borrow::Cow<'run, str>>,
}

#[derive(Default)]
struct Artifacts<'build> {
	unsigned: Option<StagedRelease<'build>>,
	current: Option<PathBuf>,
	private: Option<tempfile::TempDir>,
	retained_path: Option<PathBuf>,
}

impl Artifacts<'_> {
	fn path(&self) -> CliResult<&Path> {
		self.current.as_deref().ok_or_else(|| STAGE_MISSING.into())
	}

	fn validate_sources(&self) -> CliResult {
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

	fn retain_for_notarization(&mut self) -> CliResult {
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

fn drive<'build>(
	run: &mut ReleaseRun,
	artifacts: &mut Artifacts<'build>,
	image: HfsImage<'build>,
	settings: &ReleaseSettings<'_>,
	record: Option<&Path>,
	output: &Path,
	progress: impl Fn(BuildProgress),
) -> CliResult {
	let mut image = Some(image);
	loop {
		let observed = artifacts
			.current
			.as_deref()
			.map(release_artifact::observe)
			.transpose()?;
		let Some(action) = run.begin(observed.as_ref())? else {
			return Ok(());
		};
		let receipt = match execute(
			action, artifacts, &mut image, settings, record, output, &progress,
		) {
			Ok(receipt) => receipt,
			Err(error) => {
				let failure = error
					.downcast_ref::<release_native::NativeReleaseError>()
					.map_or(
						StageFailure::Failed,
						release_native::NativeReleaseError::stage_failure,
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
	settings: &ReleaseSettings<'_>,
	record: Option<&Path>,
	output: &Path,
	progress: &impl Fn(BuildProgress),
) -> CliResult<ReleaseReceipt> {
	match action {
		ReleaseAction::BuildUnsigned => {
			let stage = image
				.take()
				.ok_or(STAGE_MISSING)?
				.stage_release(&release_artifact::verify_stage, progress)?;
			let identity = release_artifact::observe(stage.path())?;
			artifacts.current = Some(stage.path().to_owned());
			artifacts.unsigned = Some(stage);
			Ok(ReleaseReceipt::Built(identity))
		}
		ReleaseAction::VerifyUnsignedImage(identity) => {
			if release_artifact::verify(artifacts.path()?)? != *identity {
				return Err(ReleaseError::ArtifactChanged.into());
			}
			Ok(ReleaseReceipt::UnsignedVerified(identity.clone()))
		}
		ReleaseAction::SignImage { backend, input } => {
			artifacts.validate_sources()?;
			let settings = settings
				.disk_signing
				.as_ref()
				.ok_or("missing disk-signing settings")?;
			let signed = artifacts.private_output(output, SIGNED_IMAGE)?;
			let report = release_native::sign_image(artifacts.path()?, &signed, settings)?;
			let observed = release_artifact::observe(&signed)?;
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
			let evidence = release_native::submit(
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
			let report = release_native::staple(
				artifacts.path()?,
				record.ok_or("missing notarization record")?,
				&stapled,
			)?;
			let observed = release_artifact::observe(&stapled)?;
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
			if release_artifact::verify(path)? != *identity {
				return Err(ReleaseError::ArtifactChanged.into());
			}
			if let Some(settings) = &settings.disk_signing {
				release_native::verify_image(path, settings)?;
			}
			Ok(ReleaseReceipt::Verified(identity.clone()))
		}
		ReleaseAction::AssessNativePolicy(identity) => {
			release_native::assess(artifacts.path()?)?;
			Ok(ReleaseReceipt::Assessed(identity.clone()))
		}
		ReleaseAction::Publish(identity) => {
			artifacts.validate_sources()?;
			release_artifact::publish(artifacts.path()?, identity, output)?;
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
