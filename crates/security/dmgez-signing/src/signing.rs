use std::{io, path::Path};

use serde::Serialize;
use thiserror::Error;

use crate::{Backend, Error, PolicyAssessment, ResolvedBackend};

/// Successful publication of a staged disk image with a verified signature.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct DiskSigningReport {
	pub schema_version: u16,
	pub backend: ResolvedBackend,
	pub source: String,
	pub output: String,
	/// Public SHA-1 fingerprint used to select the certificate in Keychain.
	pub identity: String,
	pub identifier: String,
	pub unsigned_sha256: String,
	pub signed_sha256: String,
	pub assessment: PolicyAssessment,
}

/// An error prevented a signed artifact from being published.
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
	#[error("unsigned image is outside the signing size limit")]
	ImageSize,
	#[error("could not prepare or publish the private stage")]
	Stage(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("could not read or write the disk image")]
	ImageIo(#[source] io::Error),
	#[error("could not preserve the source image quarantine attribute")]
	Quarantine(#[source] io::Error),
	#[error("native image checksum verification failed")]
	ImageChecksum,
	#[error("the native signing tool failed ({status})")]
	SigningFailed { status: std::process::ExitStatus },
	#[error("the staged image did not pass strict Developer ID signature verification")]
	SigningPolicy,
	#[error("the staged image did not have an unambiguous secure timestamp")]
	Timestamp,
	#[error("signing changed the unsigned image outside the appended signature")]
	ImageChanged,
	#[error(transparent)]
	Signature(#[from] Error),
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
