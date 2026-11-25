//! Admission for an explicitly requested project release.

use std::{error, fmt};

use dmgez_builder::preflight::ReleaseIntent;
use dmgez_config::{AppSigning, Backend, Notarization, Project, Signing, SigningMode, Text};
use dmgez_signing::{
	ResolvedBackend, is_valid_certificate_fingerprint, is_valid_disk_identifier,
	is_valid_keychain_profile_name,
	release_plan::{ReleaseApplications, ReleaseError, ReleasePlan},
};

const NATIVE: ResolvedBackend = ResolvedBackend::Native;

/// Release adapters the CLI can supply for this invocation.
#[derive(Clone, Copy, Default)]
pub(super) struct ReleaseCapabilities {
	pub(super) application_verification: bool,
	pub(super) disk_signing: bool,
	pub(super) ad_hoc_disk_signing: bool,
	pub(super) notarization: bool,
	pub(super) native_assessment: bool,
}

pub(super) struct ReleaseSettings<'input> {
	pub(super) plan: ReleasePlan,
	pub(super) disk_signing: Option<DiskSigning<'input>>,
	pub(super) notarization: Option<NativeNotarization<'input>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiskSigning<'input> {
	DeveloperId {
		identity: &'input str,
		identifier: &'input str,
	},
	AdHoc {
		identifier: &'input str,
	},
}

impl DiskSigning<'_> {
	fn backend(self) -> ResolvedBackend {
		match self {
			Self::DeveloperId { .. } => NATIVE,
			Self::AdHoc { .. } => ResolvedBackend::Rust,
		}
	}
}

pub(super) struct NativeNotarization<'input> {
	pub(super) keychain_profile: &'input str,
	pub(super) wait: bool,
}

impl<'input> ReleaseSettings<'input> {
	/// Maps configured release steps before opening any source, reading a key,
	/// or invoking Apple tools. The caller supplies detected adapter availability.
	pub(super) fn admit(
		project: &'input Project,
		intent: ReleaseIntent,
		identifier: Option<&'input str>,
		capabilities: ReleaseCapabilities,
	) -> Result<Option<Self>, AdmissionError> {
		if intent == ReleaseIntent::Unsigned {
			return Ok(None);
		}

		let signing = project
			.signing
			.as_ref()
			.filter(|signing| signing.enabled == Some(true));
		let notarization = project
			.notarization
			.as_ref()
			.filter(|notary| notary.enabled == Some(true));
		let policy = project.release.as_ref();
		if notarization.is_none()
			&& policy.is_some_and(|policy| policy.require_notarization == Some(true))
		{
			return Err(AdmissionError::NotarizationDisabled);
		}
		let assess_native_policy =
			policy.is_some_and(|policy| policy.require_native_assessment == Some(true));
		let disk = signing.filter(|signing| signing.dmg == Some(true));
		if disk.is_none() && identifier.is_some() {
			return Err(AdmissionError::UnexpectedIdentifier);
		}
		if disk.is_some_and(|signing| signing.mode == SigningMode::AdHoc)
			&& (notarization.is_some() || assess_native_policy)
		{
			return Err(AdmissionError::AdHocDistributionPolicy);
		}
		let applications = application_action(signing, capabilities.application_verification)?;
		let disk_signing = disk
			.map(|signing| admit_disk(signing, identifier, capabilities))
			.transpose()?;
		let plan = ReleasePlan {
			applications,
			sign_image: disk_signing.map(DiskSigning::backend),
			notarize: notarization.map(|_| NATIVE),
			staple: notarization
				.filter(|notary| notary.staple == Some(true))
				.map(|_| NATIVE),
			assess_native_policy,
		};
		// The release machine owns cross-stage prerequisites and always verifies
		// the final image. A missing `release.verify` does not change that gate.
		plan.start().map_err(AdmissionError::Plan)?;

		if assess_native_policy {
			require_native(
				Backend::Native,
				Stage::NativeAssessment,
				capabilities.native_assessment,
			)?;
		}
		let notarization = notarization
			.map(|notary| admit_notarization(notary, capabilities.notarization))
			.transpose()?;
		Ok(Some(Self {
			plan,
			disk_signing,
			notarization,
		}))
	}
}

fn application_action(
	signing: Option<&Signing>,
	available: bool,
) -> Result<ReleaseApplications, AdmissionError> {
	let Some(signing) = signing else {
		return Ok(ReleaseApplications::Preserve);
	};
	if !signing.rules.is_empty() {
		return Err(AdmissionError::ApplicationSigningUnsupported);
	}
	let default = if signing.mode == SigningMode::AdHoc {
		AppSigning::Preserve
	} else {
		AppSigning::Verify
	};
	match signing.apps.unwrap_or(default) {
		AppSigning::Preserve => Ok(ReleaseApplications::Preserve),
		AppSigning::Verify => {
			require_native(
				signing.backend.unwrap_or_default(),
				Stage::ApplicationVerification,
				available,
			)?;
			Ok(ReleaseApplications::Verify(NATIVE))
		}
		_ => Err(AdmissionError::ApplicationSigningUnsupported),
	}
}

fn admit_disk<'input>(
	signing: &'input Signing,
	identifier: Option<&'input str>,
	capabilities: ReleaseCapabilities,
) -> Result<DiskSigning<'input>, AdmissionError> {
	let backend = signing.backend.unwrap_or_default();
	let identifier = identifier.or_else(|| signing.identifier.as_ref().map(Text::as_str));
	if signing.mode == SigningMode::AdHoc {
		if !matches!(backend, Backend::Auto | Backend::Rust) || !capabilities.ad_hoc_disk_signing {
			return Err(AdmissionError::BackendUnavailable {
				stage: Stage::DiskSigning,
				requested: backend,
			});
		}
		if signing.identity.is_some() || signing.credential_ref.is_some() {
			return Err(AdmissionError::AdHocCredentials);
		}
		return Ok(DiskSigning::AdHoc {
			identifier: admit_identifier(identifier)?,
		});
	}
	require_native(backend, Stage::DiskSigning, capabilities.disk_signing)?;
	if signing.credential_ref.is_some() {
		return Err(AdmissionError::SigningCredentialReference);
	}
	let identity = signing
		.identity
		.as_ref()
		.map(Text::as_str)
		.ok_or(AdmissionError::MissingIdentity)?;
	if !is_valid_certificate_fingerprint(identity) {
		return Err(AdmissionError::InvalidIdentity);
	}
	Ok(DiskSigning::DeveloperId {
		identity,
		identifier: admit_identifier(identifier)?,
	})
}

fn admit_identifier(identifier: Option<&str>) -> Result<&str, AdmissionError> {
	let identifier = identifier.ok_or(AdmissionError::MissingIdentifier)?;
	if !is_valid_disk_identifier(identifier) {
		return Err(AdmissionError::InvalidIdentifier);
	}
	Ok(identifier)
}

fn admit_notarization(
	notary: &Notarization,
	available: bool,
) -> Result<NativeNotarization<'_>, AdmissionError> {
	require_native(
		notary.backend.unwrap_or_default(),
		Stage::Notarization,
		available,
	)?;
	let keychain_profile = notary
		.credential_ref
		.as_ref()
		.map(Text::as_str)
		.ok_or(AdmissionError::MissingNotaryProfile)?;
	if !is_valid_keychain_profile_name(keychain_profile) {
		return Err(AdmissionError::InvalidNotaryProfile);
	}
	Ok(NativeNotarization {
		keychain_profile,
		wait: notary.wait == Some(true),
	})
}

fn require_native(requested: Backend, stage: Stage, available: bool) -> Result<(), AdmissionError> {
	if matches!(requested, Backend::Auto | Backend::Native) && available {
		Ok(())
	} else {
		Err(AdmissionError::BackendUnavailable { stage, requested })
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
	ApplicationVerification,
	DiskSigning,
	Notarization,
	NativeAssessment,
}

impl fmt::Display for Stage {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::ApplicationVerification => "application signature verification",
			Self::DiskSigning => "disk image signing",
			Self::Notarization => "disk image notarization",
			Self::NativeAssessment => "native policy assessment",
		})
	}
}

/// Release admission errors retain no user-supplied identity or credential text.
#[derive(Debug)]
pub(super) enum AdmissionError {
	ApplicationSigningUnsupported,
	BackendUnavailable { stage: Stage, requested: Backend },
	NotarizationDisabled,
	UnexpectedIdentifier,
	MissingIdentifier,
	InvalidIdentifier,
	MissingIdentity,
	InvalidIdentity,
	SigningCredentialReference,
	AdHocCredentials,
	AdHocDistributionPolicy,
	MissingNotaryProfile,
	InvalidNotaryProfile,
	Plan(ReleaseError),
}

impl fmt::Display for AdmissionError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::ApplicationSigningUnsupported => formatter.write_str("release application re-signing and entitlements require a staged application adapter"),
			Self::BackendUnavailable { stage, requested } => write!(formatter, "{stage} backend {requested:?} is unavailable"),
			Self::NotarizationDisabled => formatter.write_str("release.require_notarization requires notarization.enabled = true"),
			Self::UnexpectedIdentifier => formatter.write_str("a release identifier requires signing.enabled and signing.dmg"),
			Self::MissingIdentifier => formatter.write_str("disk image signing requires signing.identifier or --identifier"),
			Self::InvalidIdentifier => formatter.write_str("invalid disk image signing identifier"),
			Self::MissingIdentity => formatter.write_str("disk image signing requires signing.identity"),
			Self::InvalidIdentity => formatter.write_str("signing.identity must be a 40-hex-digit Developer ID Application certificate fingerprint"),
			Self::SigningCredentialReference => formatter.write_str("native disk image signing selects a Keychain certificate through signing.identity and cannot resolve signing.credential_ref"),
			Self::AdHocCredentials => formatter.write_str("ad hoc disk image signing cannot use signing.identity or signing.credential_ref"),
			Self::AdHocDistributionPolicy => formatter.write_str("notarization and required native policy assessment require Developer ID signing"),
			Self::MissingNotaryProfile => formatter.write_str("native notarization requires a Keychain profile in notarization.credential_ref"),
			Self::InvalidNotaryProfile => formatter.write_str("notarization.credential_ref is not a valid Keychain profile name"),
			Self::Plan(error) => fmt::Display::fmt(error, formatter),
		}
	}
}

impl error::Error for AdmissionError {
	fn source(&self) -> Option<&(dyn error::Error + 'static)> {
		match self {
			Self::Plan(error) => Some(error),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests;
