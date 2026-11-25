//! Ordered release gates over privately staged images (D-V12).

use serde::Serialize;
use thiserror::Error;

use crate::ResolvedBackend;

const STAGE_COUNT: usize = 9;

mod identity;
mod notary_evidence;
mod plan;

pub use identity::ImageIdentity;
#[cfg(all(feature = "notarization-native", target_os = "macos"))]
pub(crate) use identity::valid_digest;
pub(crate) use identity::valid_submission_id;
pub use notary_evidence::{NotaryEvidence, NotaryOutcome};
pub use plan::ReleasePlan;

/// Policy selected for all application artifacts entering the build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseApplications {
	Preserve,
	Verify(ResolvedBackend),
	Sign(ResolvedBackend),
}

/// Ordered phases reported by a release run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseStage {
	Applications,
	BuildUnsigned,
	VerifyUnsignedImage,
	SignImage,
	Notarize,
	Staple,
	VerifyImage,
	AssessNativePolicy,
	Publish,
}

/// State of one selected or skipped release phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
	Skipped,
	Waiting,
	Running,
	Passed,
	Pending,
	Unknown,
	Failed,
	Unavailable,
	Cancelled,
}

/// Error outcome from a running adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageFailure {
	Failed,
	Unavailable,
}

/// A release phase, its selected backend, and its evidence status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Step {
	pub stage: ReleaseStage,
	pub backend: Option<ResolvedBackend>,
	pub status: StageStatus,
}

impl Step {
	const fn required(stage: ReleaseStage, backend: Option<ResolvedBackend>) -> Self {
		Self {
			stage,
			backend,
			status: StageStatus::Waiting,
		}
	}

	const fn skipped(stage: ReleaseStage) -> Self {
		Self {
			stage,
			backend: None,
			status: StageStatus::Skipped,
		}
	}

	const fn with_backend(stage: ReleaseStage, backend: Option<ResolvedBackend>) -> Self {
		if backend.is_some() {
			Self::required(stage, backend)
		} else {
			Self::skipped(stage)
		}
	}
}

/// Work admitted by the run. The adapter must consume or finish it once.
#[derive(Debug)]
pub enum ReleaseAction<'run> {
	PrepareApplications(ReleaseApplications),
	BuildUnsigned,
	VerifyUnsignedImage(&'run ImageIdentity),
	SignImage {
		backend: ResolvedBackend,
		input: &'run ImageIdentity,
	},
	Submit {
		backend: ResolvedBackend,
		input: &'run ImageIdentity,
	},
	Poll {
		backend: ResolvedBackend,
		input: &'run ImageIdentity,
		id: &'run str,
	},
	Staple {
		backend: ResolvedBackend,
		input: &'run ImageIdentity,
		id: &'run str,
	},
	VerifyImage(&'run ImageIdentity),
	AssessNativePolicy(&'run ImageIdentity),
	Publish(&'run ImageIdentity),
}

/// Successful stage evidence from the service that carried out the action.
#[derive(Debug)]
pub enum ReleaseReceipt {
	ApplicationsPrepared(ReleaseApplications),
	Built(ImageIdentity),
	UnsignedVerified(ImageIdentity),
	Signed {
		backend: ResolvedBackend,
		input: ImageIdentity,
		output: ImageIdentity,
	},
	Notarized(NotaryEvidence),
	Stapled {
		backend: ResolvedBackend,
		submission_id: String,
		input: ImageIdentity,
		output: ImageIdentity,
	},
	Verified(ImageIdentity),
	Assessed(ImageIdentity),
	Published(ImageIdentity),
}

/// Progress, including the current image digest and any remote submission.
#[derive(Debug, Serialize)]
pub struct ReleaseReport {
	pub schema_version: u16,
	pub steps: [Step; STAGE_COUNT],
	pub unsigned: Option<ImageIdentity>,
	pub artifact: Option<ImageIdentity>,
	pub notarization: Option<NotaryEvidence>,
}

/// A single authorized run. The adapter owns I/O, private stages, and records.
#[derive(Debug)]
pub struct ReleaseRun {
	applications: ReleaseApplications,
	report: ReleaseReport,
}

/// Why the requested plan or stage evidence was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ReleaseError {
	#[error("invalid image byte count or SHA-256")]
	ImageIdentity,
	#[error("a signed disk image is required for notarization or native policy assessment")]
	SignedImageRequired,
	#[error("stapling requires notarization")]
	NotarizationRequired,
	#[error("no release stage is running")]
	NoStage,
	#[error("the observed staged artifact identity changed")]
	ArtifactChanged,
	#[error("release receipt does not match the running stage, selected backend, or staged image")]
	MismatchedReceipt,
}

impl ReleaseRun {
	pub const fn report(&self) -> &ReleaseReport {
		&self.report
	}

	/// Admits exactly one required action with the observed artifact identity.
	///
	/// Observe the privately staged artifact immediately before admission and
	/// protect that snapshot through the operation. Before the unsigned build
	/// has produced an image, supply `None`. A stalled or terminal run returns
	/// `None`, and no two actions can run at once.
	///
	/// # Errors
	/// Fails and halts the run when the observed image identity differs.
	pub fn begin(
		&mut self,
		observed: Option<&ImageIdentity>,
	) -> Result<Option<ReleaseAction<'_>>, ReleaseError> {
		let Some(index) = self.active() else {
			return Ok(None);
		};
		let step = self.report.steps[index];
		if step.status != StageStatus::Waiting {
			return Ok(None);
		}
		self.check_observed(index, observed)?;
		let action = action(
			step,
			self.applications,
			self.report.artifact.as_ref(),
			self.report.notarization.as_ref(),
		)
		.ok_or(ReleaseError::NoStage)?;
		self.report.steps[index].status = StageStatus::Running;
		Ok(Some(action))
	}

	/// Resumes polling a known submission. An ambiguous upload with no ID
	/// cannot issue another submit or proceed to publication.
	///
	/// # Errors
	/// Fails and halts the run when the observed image identity differs.
	pub fn resume_notarization(
		&mut self,
		observed: &ImageIdentity,
	) -> Result<Option<ReleaseAction<'_>>, ReleaseError> {
		let Some(index) = self.active() else {
			return Ok(None);
		};
		let step = self.report.steps[index];
		if step.stage != ReleaseStage::Notarize
			|| !matches!(step.status, StageStatus::Pending | StageStatus::Unknown)
		{
			return Ok(None);
		}
		self.check_observed(index, Some(observed))?;
		let Some((evidence, id)) = self
			.report
			.notarization
			.as_ref()
			.and_then(|evidence| evidence.submission_id.as_deref().map(|id| (evidence, id)))
		else {
			return Ok(None);
		};
		self.report.steps[index].status = StageStatus::Running;
		Ok(Some(ReleaseAction::Poll {
			backend: evidence.backend,
			input: &evidence.image,
			id,
		}))
	}

	/// Completes the action with receipt evidence. An invalid receipt halts the run.
	///
	/// # Errors
	/// Returns an error for an action without prior admission or evidence of a
	/// different stage, backend, submission, or artifact.
	pub fn complete(&mut self, receipt: ReleaseReceipt) -> Result<(), ReleaseError> {
		let index = self.running()?;
		let result = self.apply(self.report.steps[index], receipt);
		self.report.steps[index].status = result.unwrap_or(StageStatus::Failed);
		result.map(|_| ())
	}

	/// Ends a running step after an adapter failure or unavailable backend.
	///
	/// # Errors
	/// Returns an error when no stage is running.
	pub fn fail(&mut self, failure: StageFailure) -> Result<(), ReleaseError> {
		let index = self.running()?;
		self.report.steps[index].status = match failure {
			StageFailure::Failed => StageStatus::Failed,
			StageFailure::Unavailable => StageStatus::Unavailable,
		};
		Ok(())
	}

	/// Stops local work and retains existing evidence. Apple can continue
	/// processing a remote submission after local cancellation.
	pub fn cancel(&mut self) {
		if let Some(index) = self.active() {
			let step = &mut self.report.steps[index];
			if matches!(
				step.status,
				StageStatus::Waiting
					| StageStatus::Running
					| StageStatus::Pending
					| StageStatus::Unknown
			) {
				step.status = StageStatus::Cancelled;
			}
		}
	}

	fn active(&self) -> Option<usize> {
		self.report
			.steps
			.iter()
			.position(|step| !matches!(step.status, StageStatus::Skipped | StageStatus::Passed))
	}

	fn running(&self) -> Result<usize, ReleaseError> {
		self.active()
			.filter(|&index| self.report.steps[index].status == StageStatus::Running)
			.ok_or(ReleaseError::NoStage)
	}

	fn check_observed(
		&mut self,
		index: usize,
		observed: Option<&ImageIdentity>,
	) -> Result<(), ReleaseError> {
		if self.report.artifact.as_ref() != observed {
			self.report.steps[index].status = StageStatus::Failed;
			return Err(ReleaseError::ArtifactChanged);
		}
		Ok(())
	}

	fn apply(&mut self, step: Step, receipt: ReleaseReceipt) -> Result<StageStatus, ReleaseError> {
		let current = self.report.artifact.as_ref();
		match (step.stage, receipt) {
			(ReleaseStage::Applications, ReleaseReceipt::ApplicationsPrepared(policy))
				if self.applications == policy => {}
			(ReleaseStage::BuildUnsigned, ReleaseReceipt::Built(image)) => {
				self.report.unsigned = Some(image.clone());
				self.report.artifact = Some(image);
			}
			(
				ReleaseStage::SignImage,
				ReleaseReceipt::Signed {
					backend,
					input,
					output,
				},
			) if step.backend == Some(backend)
				&& current == Some(&input)
				&& appends_to(&input, &output) =>
			{
				self.report.artifact = Some(output);
			}
			(ReleaseStage::Notarize, ReleaseReceipt::Notarized(evidence))
				if step.backend == Some(evidence.backend) && current == Some(&evidence.image) =>
			{
				return self.record_notary(evidence);
			}
			(
				ReleaseStage::Staple,
				ReleaseReceipt::Stapled {
					backend,
					submission_id,
					input,
					output,
				},
			) if step.backend == Some(backend)
				&& current == Some(&input)
				&& appends_to(&input, &output)
				&& self.accepts_ticket(&submission_id, &input) =>
			{
				self.report.artifact = Some(output);
			}
			(ReleaseStage::VerifyUnsignedImage, ReleaseReceipt::UnsignedVerified(image))
			| (ReleaseStage::VerifyImage, ReleaseReceipt::Verified(image))
			| (ReleaseStage::AssessNativePolicy, ReleaseReceipt::Assessed(image))
			| (ReleaseStage::Publish, ReleaseReceipt::Published(image))
				if current == Some(&image) => {}
			_ => return Err(ReleaseError::MismatchedReceipt),
		}
		Ok(StageStatus::Passed)
	}

	fn record_notary(&mut self, mut evidence: NotaryEvidence) -> Result<StageStatus, ReleaseError> {
		if let Some(id) = &mut evidence.submission_id {
			if !valid_submission_id(id) {
				return Err(ReleaseError::MismatchedReceipt);
			}
			id.make_ascii_lowercase();
		}
		let id = evidence.submission_id.as_deref();
		let previous = self
			.report
			.notarization
			.as_ref()
			.and_then(|prior| prior.submission_id.as_deref());
		if previous.is_some_and(|previous| id != Some(previous)) {
			return Err(ReleaseError::MismatchedReceipt);
		}
		let status = match evidence.outcome {
			NotaryOutcome::Unknown => StageStatus::Unknown,
			NotaryOutcome::Pending if id.is_some() => StageStatus::Pending,
			NotaryOutcome::AcceptedWithVerifiedDigest if id.is_some() => StageStatus::Passed,
			NotaryOutcome::RejectedWithVerifiedDigest if id.is_some() => StageStatus::Failed,
			_ => return Err(ReleaseError::MismatchedReceipt),
		};
		self.report.notarization = Some(evidence);
		Ok(status)
	}

	fn accepts_ticket(&self, id: &str, image: &ImageIdentity) -> bool {
		self.report.notarization.as_ref().is_some_and(|evidence| {
			evidence.outcome == NotaryOutcome::AcceptedWithVerifiedDigest
				&& evidence
					.submission_id
					.as_deref()
					.is_some_and(|bound| bound.eq_ignore_ascii_case(id))
				&& &evidence.image == image
		})
	}
}

fn action<'run>(
	step: Step,
	applications: ReleaseApplications,
	input: Option<&'run ImageIdentity>,
	notarization: Option<&'run NotaryEvidence>,
) -> Option<ReleaseAction<'run>> {
	Some(match step.stage {
		ReleaseStage::Applications => ReleaseAction::PrepareApplications(applications),
		ReleaseStage::BuildUnsigned => ReleaseAction::BuildUnsigned,
		ReleaseStage::VerifyUnsignedImage => ReleaseAction::VerifyUnsignedImage(input?),
		ReleaseStage::SignImage => ReleaseAction::SignImage {
			backend: step.backend?,
			input: input?,
		},
		ReleaseStage::Notarize => ReleaseAction::Submit {
			backend: step.backend?,
			input: input?,
		},
		ReleaseStage::Staple => ReleaseAction::Staple {
			backend: step.backend?,
			input: input?,
			id: notarization?.submission_id.as_deref()?,
		},
		ReleaseStage::VerifyImage => ReleaseAction::VerifyImage(input?),
		ReleaseStage::AssessNativePolicy => ReleaseAction::AssessNativePolicy(input?),
		ReleaseStage::Publish => ReleaseAction::Publish(input?),
	})
}

fn appends_to(input: &ImageIdentity, output: &ImageIdentity) -> bool {
	output.bytes() > input.bytes() && output.sha256() != input.sha256()
}

#[cfg(test)]
mod tests;
