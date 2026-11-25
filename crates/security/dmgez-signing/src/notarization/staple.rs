use std::path::Path;

use serde::Serialize;

use crate::{PolicyAssessment, ResolvedBackend};

use super::{Error, Id};

/// Result of checking the embedded ticket of an existing disk image.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TicketStatus {
	/// The image has no embedded ticket slot.
	Missing,
	/// The native validator reported invalid or revoked ticket data.
	Invalid,
	/// The native validator verified the embedded ticket.
	Verified,
	/// Native validation is unavailable on this platform or installation.
	Unavailable,
}

impl TicketStatus {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Missing => "missing",
			Self::Invalid => "invalid",
			Self::Verified => "verified",
			Self::Unavailable => "unavailable",
		}
	}
}

/// Checks an existing embedded disk-image ticket without modifying the image.
///
/// On macOS this checks ticket presence locally before invoking the native
/// validator, which can contact Apple's ticket service. Keep the path and
/// its containing directory stable during the check. Signature and Gatekeeper
/// checks remain separate.
///
/// # Errors
/// Fails for an unsuitable image, image changes, or native validation failures
/// that do not establish a ticket outcome.
pub fn check_ticket(image: &Path) -> Result<TicketStatus, Error> {
	#[cfg(target_os = "macos")]
	{
		crate::native::staple_image::check_ticket(image)
	}
	#[cfg(not(target_os = "macos"))]
	{
		let _ = image;
		Ok(TicketStatus::Unavailable)
	}
}

/// Publication details for an image with a validated stapled ticket.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct StapleReport {
	pub schema_version: u16,
	pub backend: ResolvedBackend,
	pub submission_id: Id,
	pub source: String,
	pub output: String,
	pub input_sha256: String,
	pub output_sha256: String,
	pub ticket_validated: bool,
	pub assessment: PolicyAssessment,
}

#[cfg(target_os = "macos")]
pub(crate) struct AcceptedImage {
	pub(crate) id: Id,
	pub(crate) digest: String,
	pub(crate) bytes: u64,
}

/// Stages a DMG whose size and SHA-256 match an accepted local record, attaches
/// its ticket, validates the staged image, and publishes it at a new path.
///
/// Use trusted local directories and leave the source and record stable during
/// the operation. Stapling explicitly contacts Apple's ticket service; the
/// ticket validator is separate from Gatekeeper policy assessment.
///
/// # Errors
/// Fails for unavailable tooling, an unsuitable record or image, mismatched
/// bytes, stapling or validation failure, or an occupied destination.
pub fn staple(image: &Path, record: &Path, output: &Path) -> Result<StapleReport, Error> {
	#[cfg(target_os = "macos")]
	{
		use super::{Status, store::Store};

		let (_, record) = Store::open(record).map_err(|source| Error::Record(Box::new(source)))?;
		let expected = match record {
			super::Report {
				status: Status::Accepted,
				digest_verified: true,
				submission_id: Some(id),
				artifact_sha256: Some(digest),
				artifact_bytes: Some(bytes),
				..
			} => AcceptedImage { id, digest, bytes },
			_ => return Err(Error::AcceptanceRequired),
		};
		crate::native::staple_image::staple(image, output, expected)
	}
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (image, record, output);
		Err(Error::Unavailable)
	}
}
