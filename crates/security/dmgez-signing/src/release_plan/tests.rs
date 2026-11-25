use super::*;

const BACKEND: ResolvedBackend = ResolvedBackend::Native;
const SUBMISSION: &str = "298227af-9bcf-4c58-ae2f-077ae4f72097";
const UNRELATED_SUBMISSION: &str = "d12964d5-6f58-45e6-8ab7-37b9bb467041";
const UNSIGNED_BYTES: u64 = 4096;
const SIGNED_BYTES: u64 = 8192;
const STAPLED_BYTES: u64 = 12_288;

fn image(digit: char, bytes: u64) -> ImageIdentity {
	let sha256: String = std::iter::repeat_n(digit, 64).collect();
	ImageIdentity::new(&sha256, bytes).unwrap()
}

fn plan() -> ReleasePlan {
	ReleasePlan {
		applications: ReleaseApplications::Sign(BACKEND),
		sign_image: Some(BACKEND),
		notarize: Some(BACKEND),
		staple: Some(BACKEND),
		assess_native_policy: true,
	}
}

fn notary(image: &ImageIdentity, id: Option<&str>, outcome: NotaryOutcome) -> ReleaseReceipt {
	ReleaseReceipt::Notarized(NotaryEvidence {
		backend: BACKEND,
		image: image.clone(),
		submission_id: id.map(str::to_owned),
		outcome,
	})
}

fn verify_unsigned(run: &mut ReleaseRun, unsigned: &ImageIdentity) {
	assert!(matches!(
		run.begin(Some(unsigned)).unwrap(),
		Some(ReleaseAction::VerifyUnsignedImage(input)) if input == unsigned
	));
	run.complete(ReleaseReceipt::UnsignedVerified(unsigned.clone()))
		.unwrap();
}

fn ready_to_submit() -> (ReleaseRun, ImageIdentity, ImageIdentity) {
	let mut run = plan().start().unwrap();
	let unsigned = image('a', UNSIGNED_BYTES);
	let signed = image('b', SIGNED_BYTES);
	let applications = ReleaseApplications::Sign(BACKEND);
	assert!(matches!(
		run.begin(None).unwrap(),
		Some(ReleaseAction::PrepareApplications(got)) if got == applications
	));
	assert!(run.begin(None).unwrap().is_none());
	run.complete(ReleaseReceipt::ApplicationsPrepared(applications))
		.unwrap();
	assert!(matches!(
		run.begin(None).unwrap(),
		Some(ReleaseAction::BuildUnsigned)
	));
	run.complete(ReleaseReceipt::Built(unsigned.clone()))
		.unwrap();
	verify_unsigned(&mut run, &unsigned);
	assert!(matches!(
		run.begin(Some(&unsigned)).unwrap(),
		Some(ReleaseAction::SignImage { backend, input }) if input == &unsigned && backend == BACKEND
	));
	run.complete(ReleaseReceipt::Signed {
		backend: BACKEND,
		input: unsigned.clone(),
		output: signed.clone(),
	})
	.unwrap();
	(run, unsigned, signed)
}

#[test]
fn passes_the_exact_stapled_image_through_final_checks_and_publication() {
	let (mut run, unsigned, signed) = ready_to_submit();
	let stapled = image('c', STAPLED_BYTES);
	let uppercase_id = SUBMISSION.to_ascii_uppercase();
	assert!(matches!(
		run.begin(Some(&signed)).unwrap(),
		Some(ReleaseAction::Submit { input, .. }) if input == &signed
	));
	run.complete(notary(&signed, Some(&uppercase_id), NotaryOutcome::Pending))
		.unwrap();
	assert!(run.begin(Some(&signed)).unwrap().is_none());
	assert!(matches!(
		run.resume_notarization(&signed).unwrap(),
		Some(ReleaseAction::Poll { input, id, .. }) if input == &signed && id == SUBMISSION
	));
	run.complete(notary(
		&signed,
		Some(SUBMISSION),
		NotaryOutcome::AcceptedWithVerifiedDigest,
	))
	.unwrap();
	assert!(matches!(
		run.begin(Some(&signed)).unwrap(),
		Some(ReleaseAction::Staple { input, id, .. }) if input == &signed && id == SUBMISSION
	));
	run.complete(ReleaseReceipt::Stapled {
		backend: BACKEND,
		submission_id: uppercase_id,
		input: signed.clone(),
		output: stapled.clone(),
	})
	.unwrap();
	assert!(matches!(
		run.begin(Some(&stapled)).unwrap(),
		Some(ReleaseAction::VerifyImage(input)) if input == &stapled
	));
	run.complete(ReleaseReceipt::Verified(stapled.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&stapled)).unwrap(),
		Some(ReleaseAction::AssessNativePolicy(input)) if input == &stapled
	));
	run.complete(ReleaseReceipt::Assessed(stapled.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&stapled)).unwrap(),
		Some(ReleaseAction::Publish(input)) if input == &stapled
	));
	run.complete(ReleaseReceipt::Published(stapled.clone()))
		.unwrap();
	assert!(run.begin(Some(&stapled)).unwrap().is_none());
	assert_eq!(run.report().unsigned.as_ref(), Some(&unsigned));
	assert_eq!(run.report().artifact.as_ref(), Some(&stapled));
	assert_eq!(run.report().notarization.as_ref().unwrap().image, signed);
	assert!(
		run.report()
			.steps
			.iter()
			.all(|step| step.status == StageStatus::Passed)
	);
}

#[test]
fn ambiguous_submission_and_rejection_cannot_be_resubmitted_or_published() {
	for (id, outcome) in [
		(None, NotaryOutcome::Unknown),
		(Some(SUBMISSION), NotaryOutcome::RejectedWithVerifiedDigest),
	] {
		let (mut run, _, signed) = ready_to_submit();
		assert!(matches!(
			run.begin(Some(&signed)).unwrap(),
			Some(ReleaseAction::Submit { .. })
		));
		run.complete(notary(&signed, id, outcome)).unwrap();
		assert!(run.resume_notarization(&signed).unwrap().is_none());
		assert!(run.begin(Some(&signed)).unwrap().is_none());
		assert_eq!(
			run.complete(ReleaseReceipt::Published(signed)),
			Err(ReleaseError::NoStage)
		);
	}
}

#[test]
fn a_resumed_receipt_cannot_change_the_digest_or_submission_id() {
	let changed = image('c', SIGNED_BYTES);
	for replaced_id in [false, true] {
		let (mut run, _, signed) = ready_to_submit();
		assert!(matches!(
			run.begin(Some(&signed)).unwrap(),
			Some(ReleaseAction::Submit { .. })
		));
		run.complete(notary(&signed, Some(SUBMISSION), NotaryOutcome::Pending))
			.unwrap();
		assert!(matches!(
			run.resume_notarization(&signed).unwrap(),
			Some(ReleaseAction::Poll { .. })
		));
		let (artifact, id) = if replaced_id {
			(&signed, UNRELATED_SUBMISSION)
		} else {
			(&changed, SUBMISSION)
		};
		assert_eq!(
			run.complete(notary(
				artifact,
				Some(id),
				NotaryOutcome::AcceptedWithVerifiedDigest
			)),
			Err(ReleaseError::MismatchedReceipt)
		);
		assert_eq!(
			run.report()
				.notarization
				.as_ref()
				.unwrap()
				.submission_id
				.as_deref(),
			Some(SUBMISSION)
		);
		assert!(run.begin(Some(&signed)).unwrap().is_none());
	}
}

#[test]
fn an_image_change_prevents_publication_admission() {
	let mut unsigned_plan = plan();
	unsigned_plan.applications = ReleaseApplications::Preserve;
	unsigned_plan.sign_image = None;
	unsigned_plan.notarize = None;
	unsigned_plan.staple = None;
	unsigned_plan.assess_native_policy = false;
	let mut run = unsigned_plan.start().unwrap();
	let original = image('a', UNSIGNED_BYTES);
	let changed = image('a', SIGNED_BYTES);
	assert!(matches!(
		run.begin(None).unwrap(),
		Some(ReleaseAction::BuildUnsigned)
	));
	run.complete(ReleaseReceipt::Built(original.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&original)).unwrap(),
		Some(ReleaseAction::VerifyImage(_))
	));
	run.complete(ReleaseReceipt::Verified(original.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&changed)),
		Err(ReleaseError::ArtifactChanged)
	));
	assert!(run.begin(Some(&original)).unwrap().is_none());
}

#[test]
fn cancellation_preserves_the_bound_submission_and_blocks_further_actions() {
	let (mut run, _, signed) = ready_to_submit();
	assert!(matches!(
		run.begin(Some(&signed)).unwrap(),
		Some(ReleaseAction::Submit { .. })
	));
	run.complete(notary(&signed, Some(SUBMISSION), NotaryOutcome::Pending))
		.unwrap();
	run.cancel();
	assert_eq!(
		run.report().notarization.as_ref().unwrap().outcome,
		NotaryOutcome::Pending
	);
	assert!(run.resume_notarization(&signed).unwrap().is_none());
	assert!(run.begin(Some(&signed)).unwrap().is_none());
}

#[test]
fn unavailable_policy_blocks_release_and_impossible_plans_fail_before_work() {
	let mut impossible = plan();
	impossible.sign_image = None;
	assert_eq!(
		impossible.start().unwrap_err(),
		ReleaseError::SignedImageRequired
	);
	impossible = plan();
	impossible.notarize = None;
	assert_eq!(
		impossible.start().unwrap_err(),
		ReleaseError::NotarizationRequired
	);

	let mut unsigned_plan = plan();
	unsigned_plan.applications = ReleaseApplications::Preserve;
	unsigned_plan.notarize = None;
	unsigned_plan.staple = None;
	let mut run = unsigned_plan.start().unwrap();
	let unsigned = image('a', UNSIGNED_BYTES);
	let signed = image('b', SIGNED_BYTES);
	assert!(matches!(
		run.begin(None).unwrap(),
		Some(ReleaseAction::BuildUnsigned)
	));
	run.complete(ReleaseReceipt::Built(unsigned.clone()))
		.unwrap();
	verify_unsigned(&mut run, &unsigned);
	assert!(matches!(
		run.begin(Some(&unsigned)).unwrap(),
		Some(ReleaseAction::SignImage { .. })
	));
	run.complete(ReleaseReceipt::Signed {
		backend: BACKEND,
		input: unsigned,
		output: signed.clone(),
	})
	.unwrap();
	assert!(matches!(
		run.begin(Some(&signed)).unwrap(),
		Some(ReleaseAction::VerifyImage(_))
	));
	run.complete(ReleaseReceipt::Verified(signed.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&signed)).unwrap(),
		Some(ReleaseAction::AssessNativePolicy(_))
	));
	run.fail(StageFailure::Unavailable).unwrap();
	assert!(run.begin(Some(&signed)).unwrap().is_none());
}

#[test]
fn signing_cannot_replace_unsigned_image_verification() {
	let mut plan = plan();
	plan.applications = ReleaseApplications::Preserve;
	let mut run = plan.start().unwrap();
	let unsigned = image('a', UNSIGNED_BYTES);
	let signed = image('b', SIGNED_BYTES);
	assert!(matches!(
		run.begin(None).unwrap(),
		Some(ReleaseAction::BuildUnsigned)
	));
	run.complete(ReleaseReceipt::Built(unsigned.clone()))
		.unwrap();
	assert!(matches!(
		run.begin(Some(&unsigned)).unwrap(),
		Some(ReleaseAction::VerifyUnsignedImage(_))
	));
	assert_eq!(
		run.complete(ReleaseReceipt::Signed {
			backend: BACKEND,
			input: unsigned.clone(),
			output: signed,
		}),
		Err(ReleaseError::MismatchedReceipt)
	);
	assert!(run.begin(Some(&unsigned)).unwrap().is_none());
}

#[cfg(feature = "notarization-native")]
#[test]
fn a_notary_report_requires_digest_verified_acceptance_evidence() {
	use crate::notarization::{LogSummary, Report, Status};

	let signed = image('b', SIGNED_BYTES);
	let mut report = Report {
		schema_version: 1,
		backend: BACKEND,
		submission_id: Some(SUBMISSION.parse().unwrap()),
		status: Status::Accepted,
		artifact_sha256: Some(signed.sha256().to_owned()),
		artifact_bytes: Some(signed.bytes()),
		created_at: 0,
		updated_at: 1,
		digest_verified: false,
		log: Some(LogSummary {
			errors: 0,
			warnings: 0,
		}),
		issue: None,
	};
	assert_eq!(
		NotaryEvidence::try_from(&report),
		Err(ReleaseError::MismatchedReceipt)
	);
	report.digest_verified = true;
	let accepted = NotaryEvidence::try_from(&report).unwrap();
	assert_eq!(accepted.image, signed);
	assert_eq!(accepted.outcome, NotaryOutcome::AcceptedWithVerifiedDigest);
}
