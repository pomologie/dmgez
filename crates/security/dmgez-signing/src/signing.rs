use std::{io, path::Path};

use serde::Serialize;
use thiserror::Error;

use crate::{Backend, Error, PolicyAssessment, ResolvedBackend};

#[cfg(all(
	feature = "rust",
	any(target_os = "linux", target_os = "macos", windows)
))]
pub(super) mod apple;

pub(super) const SIGNING_REPORT_VERSION: u16 = 1;

/// An entitlements plist scoped to one exact app-relative code path.
#[derive(Clone, Copy, Debug)]
pub struct AppEntitlements<'a> {
	/// A relative signing target. Use `.` for the root app.
	pub item: &'a Path,
	pub plist: &'a Path,
}

/// One code item signed and verified in the app stage.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct AppSigningItemReport {
	pub item: String,
	pub hardened_runtime: bool,
	pub entitlements: bool,
}

/// Successful publication of a verified signed app bundle.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct AppSigningReport {
	pub schema_version: u16,
	pub backend: ResolvedBackend,
	pub source: String,
	pub output: String,
	/// Public SHA-1 fingerprint of the selected certificate.
	pub identity: String,
	/// Code items in signing order.
	pub items: Vec<AppSigningItemReport>,
	pub assessment: PolicyAssessment,
}

/// Successful publication of a staged disk image with a verified signature.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct DiskSigningReport {
	pub schema_version: u16,
	pub backend: ResolvedBackend,
	pub mode: DiskSigningMode,
	pub source: String,
	pub output: String,
	/// Public SHA-1 fingerprint used to select a Developer ID certificate, when present.
	pub identity: Option<String>,
	pub identifier: String,
	pub unsigned_sha256: String,
	pub signed_sha256: String,
	pub assessment: PolicyAssessment,
}

/// Signature profile used for the staged disk image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DiskSigningMode {
	AdHoc,
	DeveloperId,
}

impl DiskSigningMode {
	/// Stable report name.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::AdHoc => "ad_hoc",
			Self::DeveloperId => "developer_id",
		}
	}
}

/// A signing or signed-artifact verification operation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SignError {
	#[error("requested {backend:?} signing backend is unavailable")]
	BackendUnavailable { backend: Backend },
	#[error("identity must be the 40-hex-digit Developer ID Application certificate fingerprint")]
	Fingerprint,
	#[error(
		"identifier must contain 1 to 255 ASCII letters, digits, periods, hyphens or underscores and start with a letter or digit"
	)]
	Identifier,
	#[error("the selected Developer ID Application identity is unavailable in Keychain")]
	IdentityUnavailable,
	#[error("could not check available Developer ID Application identities")]
	IdentityLookup(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("the source image must have an unsigned signature state")]
	ExpectedUnsigned,
	#[error("the disk image is unsigned")]
	Unsigned,
	#[error("the disk image lacks an admitted ad hoc signature")]
	AdHocProfile,
	#[error("the ad hoc disk image identifier does not match the requested identifier")]
	AdHocIdentifier,
	#[error("apple-codesign could not write the ad hoc disk image signature")]
	AppleCodesign,
	#[error("disk image is outside the signing size limit")]
	ImageSize,
	#[error("could not prepare or publish the private stage")]
	Stage(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("could not read or write the disk image")]
	ImageIo(#[source] io::Error),
	#[error("could not preserve the source image quarantine attribute")]
	Quarantine(#[source] io::Error),
	#[error("native image checksum verification failed")]
	ImageChecksum,
	#[error("the disk image did not pass integrity checks")]
	ImageIntegrity(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("the native signing tool failed ({status})")]
	SigningFailed { status: std::process::ExitStatus },
	#[error("the artifact did not pass strict Developer ID signature verification")]
	SigningPolicy,
	#[error("the artifact did not have an unambiguous secure timestamp")]
	Timestamp,
	#[error("the executable did not have the hardened runtime flag")]
	HardenedRuntime,
	#[error("could not copy or revalidate the application bundle")]
	AppCopy(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("could not plan the application's nested code")]
	AppPlan(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("a staged executable's architecture set changed during signing")]
	AppArchitectureChanged,
	#[error("could not prepare or verify the app's entitlements")]
	AppEntitlements(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("signing changed the unsigned image outside the appended signature")]
	ImageChanged,
	#[error(transparent)]
	Signature(#[from] Error),
}

/// Signs an unsigned UDIF image without a certificate and publishes a verified copy.
///
/// `Auto` selects the portable Rust signer. The source and any existing destination
/// are preserved. Use a stable local source and a trusted destination directory.
/// The output carries a verified ad hoc CodeDirectory and no publisher identity.
///
/// # Errors
/// Returns an error for an unavailable backend, invalid identifier or artifact,
/// failed staging, signing or verification, or a publication conflict.
pub fn sign_ad_hoc_disk_image(
	source: &Path,
	destination: &Path,
	identifier: &str,
	backend: Backend,
) -> Result<DiskSigningReport, SignError> {
	#[cfg(all(
		feature = "rust",
		any(target_os = "linux", target_os = "macos", windows)
	))]
	if matches!(backend, Backend::Auto | Backend::Rust) {
		return apple::sign(source, destination, identifier);
	}
	let _ = (source, destination, identifier);
	Err(SignError::BackendUnavailable { backend })
}

/// Verifies a staged or existing ad hoc image and its exact signing identifier.
///
/// `Auto` selects the portable Rust profile. This checks signature integrity and
/// the ad hoc mode, while platform policy assessment remains a separate action.
///
/// # Errors
/// Returns an error for an unavailable backend, invalid identifier or artifact,
/// an unexpected signature profile, or failed integrity checks.
pub fn verify_ad_hoc_disk_image(
	path: &Path,
	identifier: &str,
	backend: Backend,
) -> Result<(), SignError> {
	#[cfg(all(
		feature = "rust",
		any(target_os = "linux", target_os = "macos", windows)
	))]
	if matches!(backend, Backend::Auto | Backend::Rust) {
		return apple::verify(path, identifier);
	}
	let _ = (path, identifier);
	Err(SignError::BackendUnavailable { backend })
}

/// Signs an unsigned disk image and publishes it to a new output path.
///
/// The native backend uses the selected Developer ID Application identity in
/// Keychain and requests an Apple secure timestamp. The output is staged
/// privately beside the destination. The source and an existing destination
/// are left untouched. Use a stable, trusted local destination directory.
///
/// # Errors
///
/// Returns an error for an unavailable backend, invalid request or artifact,
/// staging, identity and signing failures, or failed verification.
pub fn sign_disk_image(
	source: &Path,
	destination: &Path,
	identity: &str,
	identifier: &str,
	backend: Backend,
) -> Result<DiskSigningReport, SignError> {
	#[cfg(all(feature = "native", target_os = "macos"))]
	if backend != Backend::Rust {
		return crate::native::disk::sign(source, destination, identity, identifier);
	}
	let _ = (source, destination, identity, identifier);
	Err(SignError::BackendUnavailable { backend })
}

/// Revalidates a signed disk image against an explicit Developer ID signer.
///
/// The native backend verifies the Apple Developer ID Application requirement,
/// public certificate fingerprint, identifier, secure timestamp, and image
/// checksum. Keep the image stable and in a trusted directory during the check.
/// It requires no private key. Ticket and platform policy checks are separate.
///
/// # Errors
///
/// Fails when the backend is unavailable, the image or selector is invalid,
/// the image changes during inspection, or a verification check fails.
pub fn verify_signed_disk_image(
	path: &Path,
	identity: &str,
	identifier: &str,
	backend: Backend,
) -> Result<(), SignError> {
	#[cfg(all(feature = "native", target_os = "macos"))]
	if backend != Backend::Rust {
		return crate::native::disk::verify(path, identity, identifier);
	}
	let _ = (path, identity, identifier);
	Err(SignError::BackendUnavailable { backend })
}

/// Verifies an existing app bundle against the native Developer ID Application policy.
///
/// Checks the root and admitted nested code for valid signatures, secure
/// timestamps, and hardened runtime on eligible executables. No private key
/// is required. Keep the bundle and its pathname stable during verification.
/// Bundle stamps and link targets are checked before and after inspection.
/// Gatekeeper and any caller-specific entitlement policy are separate checks.
/// # Errors
/// Fails when the backend is unavailable, the bundle cannot be admitted,
/// the observed bundle changes, or the nested code fails verification.
pub fn verify_application(path: &Path, backend: Backend) -> Result<(), SignError> {
	#[cfg(all(feature = "native", target_os = "macos"))]
	if backend != Backend::Rust {
		return crate::native::app::verify(path);
	}
	let _ = path;
	Err(SignError::BackendUnavailable { backend })
}

/// Signs an application in a private copy and publishes a verified bundle.
///
/// The native backend uses the selected Developer ID Application identity in
/// Keychain. Each nested code item is signed before its enclosing bundle.
/// Use a stable local source and a trusted local destination directory. This
/// explicit operation can replace signatures on the copy.
///
/// # Errors
///
/// Returns an error for an unavailable backend, an invalid input or rule, an
/// identity or signing failure, a verification failure, or a publication error.
pub fn sign_application(
	source: &Path,
	destination: &Path,
	identity: &str,
	entitlements: &[AppEntitlements<'_>],
	backend: Backend,
) -> Result<AppSigningReport, SignError> {
	#[cfg(all(feature = "native", target_os = "macos"))]
	if backend != Backend::Rust {
		return crate::native::app::sign(source, destination, identity, entitlements);
	}
	let _ = (source, destination, identity, entitlements);
	Err(SignError::BackendUnavailable { backend })
}
