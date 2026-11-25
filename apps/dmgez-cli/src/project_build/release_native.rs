//! Native adapters for the private project release stages.

use std::{error, fmt, path::Path};

use dmgez_signing::{
	ArtifactKind, Backend, DiskSigningReport, NativeAssessmentError, NativeAssessmentStatus,
	NativeAssessmentTool, NativeAssessmentUnavailable, ResolvedBackend, SignError,
	assess_native_policy,
	release_plan::{NotaryEvidence, StageFailure},
	sign_disk_image, verify_signed_disk_image,
};

use super::release_settings::{NativeDiskSigning, NativeNotarization};

#[cfg(all(
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
))]
pub(super) use super::release_applications::{
	prepare as prepare_applications, verify_unsigned as verify_applications,
};

#[cfg(not(all(
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
)))]
pub(super) use unavailable::{prepare_applications, verify_applications};

#[cfg(not(all(
	feature = "signing-native",
	feature = "hfsplus-install",
	target_os = "macos"
)))]
mod unavailable {
	use dmgez_builder::planning::PreparedBuild;
	use dmgez_config::VirtualPath;
	use dmgez_signing::release_plan::ImageIdentity;

	use crate::CliResult;

	use super::{Backend, NativeReleaseError, Path, ResolvedBackend, SignError};

	pub(in crate::project_build) fn prepare_applications(
		_prepared: &PreparedBuild,
		_backend: ResolvedBackend,
	) -> CliResult<Vec<VirtualPath>> {
		Err(unavailable())
	}

	pub(in crate::project_build) fn verify_applications(
		_path: &Path,
		_expected: &ImageIdentity,
		_prepared: &PreparedBuild,
		_applications: &[VirtualPath],
		_backend: ResolvedBackend,
	) -> CliResult<usize> {
		Err(unavailable())
	}

	fn unavailable() -> Box<dyn std::error::Error> {
		NativeReleaseError::Signing(SignError::BackendUnavailable {
			backend: Backend::Native,
		})
		.into()
	}
}

pub(super) fn sign_image(
	source: &Path,
	destination: &Path,
	settings: &NativeDiskSigning<'_>,
) -> Result<DiskSigningReport, NativeReleaseError> {
	sign_disk_image(
		source,
		destination,
		settings.identity,
		settings.identifier,
		Backend::Native,
	)
	.map_err(NativeReleaseError::Signing)
}

pub(super) fn verify_image(
	image: &Path,
	settings: &NativeDiskSigning<'_>,
) -> Result<(), NativeReleaseError> {
	verify_signed_disk_image(
		image,
		settings.identity,
		settings.identifier,
		Backend::Native,
	)
	.map_err(NativeReleaseError::Signing)
}

pub(super) fn assess(image: &Path) -> Result<(), NativeReleaseError> {
	let report = assess_native_policy(image).map_err(NativeReleaseError::Assessment)?;
	match report.outcome {
		NativeAssessmentStatus::Accepted
			if report.kind == Some(ArtifactKind::DiskImage)
				&& report.tool == Some(NativeAssessmentTool::Spctl) =>
		{
			Ok(())
		}
		NativeAssessmentStatus::Rejected => Err(NativeReleaseError::PolicyRejected),
		NativeAssessmentStatus::Unavailable(reason) => {
			Err(NativeReleaseError::PolicyUnavailable(reason))
		}
		_ => Err(NativeReleaseError::InvalidEvidence),
	}
}

#[cfg(feature = "notarization-native")]
pub(super) fn submit(
	image: &Path,
	record: &Path,
	settings: &NativeNotarization<'_>,
) -> Result<NotaryEvidence, NativeReleaseError> {
	let report = dmgez_signing::notarization::submit(
		image,
		record,
		settings.keychain_profile,
		settings.wait,
	)
	.map_err(NativeReleaseError::Notarization)?;
	NotaryEvidence::try_from(&report).map_err(|_| NativeReleaseError::InvalidEvidence)
}

#[cfg(not(feature = "notarization-native"))]
pub(super) fn submit(
	_image: &Path,
	_record: &Path,
	settings: &NativeNotarization<'_>,
) -> Result<NotaryEvidence, NativeReleaseError> {
	let _ = (settings.keychain_profile, settings.wait);
	Err(NativeReleaseError::NotarizationUnavailable)
}

pub(super) struct NativeStapleReport {
	pub(super) backend: ResolvedBackend,
	pub(super) submission_id: String,
	pub(super) input_sha256: String,
	pub(super) output_sha256: String,
}

#[cfg(feature = "notarization-native")]
pub(super) fn staple(
	image: &Path,
	record: &Path,
	destination: &Path,
) -> Result<NativeStapleReport, NativeReleaseError> {
	let report = dmgez_signing::notarization::staple(image, record, destination)
		.map_err(NativeReleaseError::Notarization)?;
	if !report.ticket_validated {
		return Err(NativeReleaseError::InvalidEvidence);
	}
	Ok(NativeStapleReport {
		backend: report.backend,
		submission_id: report.submission_id.to_string(),
		input_sha256: report.input_sha256,
		output_sha256: report.output_sha256,
	})
}

#[cfg(not(feature = "notarization-native"))]
pub(super) fn staple(
	_image: &Path,
	_record: &Path,
	_destination: &Path,
) -> Result<NativeStapleReport, NativeReleaseError> {
	Err(NativeReleaseError::NotarizationUnavailable)
}

#[derive(Debug)]
pub(super) enum NativeReleaseError {
	Signing(SignError),
	Assessment(NativeAssessmentError),
	PolicyRejected,
	PolicyUnavailable(NativeAssessmentUnavailable),
	InvalidEvidence,
	#[cfg(feature = "notarization-native")]
	Notarization(dmgez_signing::notarization::Error),
	#[cfg(not(feature = "notarization-native"))]
	NotarizationUnavailable,
}

impl NativeReleaseError {
	pub(super) fn stage_failure(&self) -> StageFailure {
		match self {
			Self::Signing(
				SignError::BackendUnavailable { .. } | SignError::IdentityUnavailable,
			)
			| Self::PolicyUnavailable(_) => StageFailure::Unavailable,
			#[cfg(not(feature = "notarization-native"))]
			Self::NotarizationUnavailable => StageFailure::Unavailable,
			// The admitted profile has already passed syntax validation, so a
			// tool initialization error means the local backend is unavailable.
			#[cfg(feature = "notarization-native")]
			Self::Notarization(
				dmgez_signing::notarization::Error::Unavailable
				| dmgez_signing::notarization::Error::Tool(_),
			) => StageFailure::Unavailable,
			_ => StageFailure::Failed,
		}
	}
}

impl fmt::Display for NativeReleaseError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Signing(error) => fmt::Display::fmt(error, formatter),
			Self::Assessment(error) => fmt::Display::fmt(error, formatter),
			Self::PolicyRejected => formatter.write_str("native policy rejected the release image"),
			Self::PolicyUnavailable(reason) => write!(
				formatter,
				"native policy assessment unavailable ({})",
				reason.as_str()
			),
			Self::InvalidEvidence => {
				formatter.write_str("native tool did not return the required release evidence")
			}
			#[cfg(feature = "notarization-native")]
			Self::Notarization(error) => fmt::Display::fmt(error, formatter),
			#[cfg(not(feature = "notarization-native"))]
			Self::NotarizationUnavailable => formatter.write_str("native notarization was not compiled"),
		}
	}
}

impl error::Error for NativeReleaseError {
	fn source(&self) -> Option<&(dyn error::Error + 'static)> {
		match self {
			Self::Signing(error) => Some(error),
			Self::Assessment(error) => Some(error),
			#[cfg(feature = "notarization-native")]
			Self::Notarization(error) => Some(error),
			_ => None,
		}
	}
}
