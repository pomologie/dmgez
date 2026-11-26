use dmgez_signing::release_plan::{ReleaseStage, StageStatus};

use super::*;

const MINIMAL_PROJECT: &str =
	"schema_version = 1\n[image]\nname = 'Fixture'\noutput = 'fixture.dmg'";
const CERTIFICATE_FINGERPRINT: &str = "0123456789abcdef0123456789abcdef01234567";
const DISK_IDENTIFIER: &str = "org.dmgez.release-fixture";
const NOTARY_PROFILE: &str = "synthetic release profile";
const APP_AND_DISK: &str = "[signing]\nenabled = true\ndmg = true";
const PRESERVE_AND_DISK: &str = "[signing]\nenabled = true\napps = 'preserve'\ndmg = true";
const AD_HOC_DISK: &str = "[signing]\nenabled = true\ndmg = true\nmode = 'ad-hoc'";

fn project(release_sections: &str) -> Project {
	Project::parse(&format!("{MINIMAL_PROJECT}\n{release_sections}\n")).unwrap()
}

fn native_capabilities() -> ReleaseCapabilities {
	ReleaseCapabilities {
		application_verification: true,
		disk_signing: true,
		ad_hoc_disk_signing: true,
		notarization: true,
		native_assessment: true,
	}
}

#[test]
fn ordinary_build_ignores_even_unavailable_settings() {
	let source = project(&format!(
		"{APP_AND_DISK}\nbackend = 'rust'\n[release]\nrequire_notarization = true"
	));
	assert!(
		ReleaseSettings::admit(
			&source,
			ReleaseIntent::Unsigned,
			None,
			ReleaseCapabilities::default(),
		)
		.unwrap()
		.is_none()
	);
}

#[test]
fn native_signing_and_notarization_admit_exact_selectors_and_all_required_stages() {
	let source = project(&format!(
		"{APP_AND_DISK}\nidentity = '{CERTIFICATE_FINGERPRINT}'\n\
		[notarization]\nenabled = true\nbackend = 'native'\nwait = true\nstaple = true\ncredential_ref = '{NOTARY_PROFILE}'\n\
		[release]\nverify = true\nrequire_notarization = true\nrequire_native_assessment = true"
	));
	let settings = ReleaseSettings::admit(
		&source,
		ReleaseIntent::Release,
		Some(DISK_IDENTIFIER),
		native_capabilities(),
	)
	.unwrap()
	.unwrap();
	assert_eq!(
		settings.plan.applications,
		ReleaseApplications::Verify(NATIVE)
	);
	assert_eq!(settings.plan.sign_image, Some(NATIVE));
	assert_eq!(settings.plan.notarize, Some(NATIVE));
	assert_eq!(settings.plan.staple, Some(NATIVE));
	assert!(settings.plan.assess_native_policy);
	assert_eq!(
		settings.disk_signing,
		Some(DiskSigning::DeveloperId {
			identity: CERTIFICATE_FINGERPRINT,
			identifier: DISK_IDENTIFIER,
		})
	);
	let notary = settings.notarization.unwrap();
	assert_eq!(notary.keychain_profile, NOTARY_PROFILE);
	assert!(notary.wait);
	let release = settings.plan.start().unwrap();
	assert!(
		release
			.report()
			.steps
			.iter()
			.all(|step| step.status == StageStatus::Waiting)
	);
}

#[test]
fn ad_hoc_projects_choose_rust_and_an_explicit_cli_identifier_overrides_the_manifest() {
	const OVERRIDE_IDENTIFIER: &str = "org.dmgez.release-override";
	let source = project(&format!("{AD_HOC_DISK}\nidentifier = '{DISK_IDENTIFIER}'"));
	for (argument, expected) in [
		(None, DISK_IDENTIFIER),
		(Some(OVERRIDE_IDENTIFIER), OVERRIDE_IDENTIFIER),
	] {
		let settings = ReleaseSettings::admit(
			&source,
			ReleaseIntent::Release,
			argument,
			native_capabilities(),
		)
		.unwrap()
		.unwrap();
		assert_eq!(settings.plan.applications, ReleaseApplications::Preserve);
		assert_eq!(settings.plan.sign_image, Some(ResolvedBackend::Rust));
		assert_eq!(
			settings.disk_signing,
			Some(DiskSigning::AdHoc {
				identifier: expected
			})
		);
	}
}

#[test]
fn ad_hoc_projects_refuse_native_selection_credentials_and_distribution_policy() {
	let admit = |extra: &str| {
		let source = project(&format!(
			"{AD_HOC_DISK}\nidentifier = '{DISK_IDENTIFIER}'\n{extra}"
		));
		ReleaseSettings::admit(&source, ReleaseIntent::Release, None, native_capabilities()).err()
	};
	assert!(matches!(
		admit("backend = 'native'"),
		Some(AdmissionError::BackendUnavailable {
			stage: Stage::DiskSigning,
			requested: Backend::Native
		})
	));
	assert!(matches!(
		admit("identity = 'identity'"),
		Some(AdmissionError::AdHocCredentials)
	));
	assert!(matches!(
		admit("credential_ref = 'reference'"),
		Some(AdmissionError::AdHocCredentials)
	));
	assert!(matches!(
		admit("[notarization]\nenabled = true"),
		Some(AdmissionError::AdHocDistributionPolicy)
	));
	assert!(matches!(
		admit("[release]\nrequire_native_assessment = true"),
		Some(AdmissionError::AdHocDistributionPolicy)
	));
}

#[test]
fn ad_hoc_projects_require_the_portable_backend_to_be_available() {
	let source = project(&format!("{AD_HOC_DISK}\nidentifier = '{DISK_IDENTIFIER}'"));
	assert!(matches!(
		ReleaseSettings::admit(
			&source,
			ReleaseIntent::Release,
			None,
			ReleaseCapabilities::default(),
		),
		Err(AdmissionError::BackendUnavailable {
			stage: Stage::DiskSigning,
			requested: Backend::Auto,
		})
	));
}

#[test]
fn unsigned_release_requires_final_verification_without_credentials_or_apple_tools() {
	let source = project("[release]\nverify = false");
	let settings = ReleaseSettings::admit(
		&source,
		ReleaseIntent::Release,
		None,
		ReleaseCapabilities::default(),
	)
	.unwrap()
	.unwrap();
	assert!(settings.disk_signing.is_none());
	assert!(settings.notarization.is_none());
	let release = settings.plan.start().unwrap();
	let required: Vec<_> = release
		.report()
		.steps
		.iter()
		.filter(|step| step.status == StageStatus::Waiting)
		.map(|step| step.stage)
		.collect();
	assert_eq!(
		required,
		[
			ReleaseStage::BuildUnsigned,
			ReleaseStage::VerifyImage,
			ReleaseStage::Publish
		]
	);
}

#[test]
fn default_application_verification_does_not_fall_back_to_preserve() {
	let source = project("[signing]\nenabled = true");
	assert!(matches!(
		ReleaseSettings::admit(
			&source,
			ReleaseIntent::Release,
			None,
			ReleaseCapabilities::default()
		),
		Err(AdmissionError::BackendUnavailable {
			stage: Stage::ApplicationVerification,
			requested: Backend::Auto,
		})
	));
}

#[test]
fn unsupported_backend_and_re_signing_requests_are_not_ignored() {
	let rust = project(&format!("{PRESERVE_AND_DISK}\nbackend = 'rust'"));
	assert!(matches!(
		ReleaseSettings::admit(&rust, ReleaseIntent::Release, None, native_capabilities()),
		Err(AdmissionError::BackendUnavailable {
			stage: Stage::DiskSigning,
			requested: Backend::Rust,
		})
	));
	for signing in [
		"[signing]\nenabled = true\napps = 'sign'",
		"[signing]\nenabled = true\napps = 'preserve'\n[[signing.rules]]\npath = '/Example.app'\nentitlements = 'unused.plist'",
	] {
		let source = project(signing);
		assert!(matches!(
			ReleaseSettings::admit(&source, ReleaseIntent::Release, None, native_capabilities()),
			Err(AdmissionError::ApplicationSigningUnsupported)
		));
	}
}

#[test]
fn required_notarization_and_assessment_do_not_run_without_prerequisites() {
	let disabled =
		project("[notarization]\nenabled = false\n[release]\nrequire_notarization = true");
	assert!(matches!(
		ReleaseSettings::admit(
			&disabled,
			ReleaseIntent::Release,
			None,
			native_capabilities()
		),
		Err(AdmissionError::NotarizationDisabled)
	));
	for missing_signature in [
		"[notarization]\nenabled = true",
		"[release]\nrequire_native_assessment = true",
	] {
		let source = project(missing_signature);
		assert!(matches!(
			ReleaseSettings::admit(&source, ReleaseIntent::Release, None, native_capabilities()),
			Err(AdmissionError::Plan(ReleaseError::SignedImageRequired))
		));
	}
}

#[test]
fn native_disk_requires_a_public_fingerprint_and_explicit_identifier() {
	let missing = project(PRESERVE_AND_DISK);
	assert!(matches!(
		ReleaseSettings::admit(
			&missing,
			ReleaseIntent::Release,
			Some(DISK_IDENTIFIER),
			native_capabilities()
		),
		Err(AdmissionError::MissingIdentity)
	));
	let bad = project(&format!(
		"{PRESERVE_AND_DISK}\nidentity = 'certificate label'"
	));
	assert!(matches!(
		ReleaseSettings::admit(
			&bad,
			ReleaseIntent::Release,
			Some(DISK_IDENTIFIER),
			native_capabilities()
		),
		Err(AdmissionError::InvalidIdentity)
	));
	let present = project(&format!(
		"{PRESERVE_AND_DISK}\nidentity = '{CERTIFICATE_FINGERPRINT}'"
	));
	assert!(matches!(
		ReleaseSettings::admit(
			&present,
			ReleaseIntent::Release,
			None,
			native_capabilities()
		),
		Err(AdmissionError::MissingIdentifier)
	));
	assert!(matches!(
		ReleaseSettings::admit(
			&present,
			ReleaseIntent::Release,
			Some("-invalid"),
			native_capabilities()
		),
		Err(AdmissionError::InvalidIdentifier)
	));
	let unsigned = project("");
	assert!(matches!(
		ReleaseSettings::admit(
			&unsigned,
			ReleaseIntent::Release,
			Some(DISK_IDENTIFIER),
			native_capabilities()
		),
		Err(AdmissionError::UnexpectedIdentifier)
	));
}

#[test]
fn unknown_and_invalid_credential_references_do_not_appear_in_errors() {
	const REDACTED_REF: &str = "synthetic opaque reference";
	let external_signing = project(&format!(
		"{PRESERVE_AND_DISK}\ncredential_ref = '{REDACTED_REF}'"
	));
	let Err(error @ AdmissionError::SigningCredentialReference) = ReleaseSettings::admit(
		&external_signing,
		ReleaseIntent::Release,
		Some(DISK_IDENTIFIER),
		native_capabilities(),
	) else {
		panic!("expected unsupported signing reference");
	};
	assert!(!format!("{error:?} {error}").contains(REDACTED_REF));

	let disk_and_notary = format!(
		"{PRESERVE_AND_DISK}\nidentity = '{CERTIFICATE_FINGERPRINT}'\n[notarization]\nenabled = true"
	);
	let missing = project(&disk_and_notary);
	assert!(matches!(
		ReleaseSettings::admit(
			&missing,
			ReleaseIntent::Release,
			Some(DISK_IDENTIFIER),
			native_capabilities()
		),
		Err(AdmissionError::MissingNotaryProfile)
	));
	let malformed = project(&format!(
		"{disk_and_notary}\ncredential_ref = \"{REDACTED_REF}\\n\""
	));
	let Err(error @ AdmissionError::InvalidNotaryProfile) = ReleaseSettings::admit(
		&malformed,
		ReleaseIntent::Release,
		Some(DISK_IDENTIFIER),
		native_capabilities(),
	) else {
		panic!("expected invalid Keychain profile");
	};
	assert!(!format!("{error:?} {error}").contains(REDACTED_REF));
}
