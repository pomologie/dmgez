use serde::Serialize;

use crate::ResolvedBackend;

use super::ImageIdentity;

/// Evidence returned by the selected notarization adapter for the exact image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NotaryEvidence {
	pub backend: ResolvedBackend,
	pub image: ImageIdentity,
	pub submission_id: Option<String>,
	pub outcome: NotaryOutcome,
}

/// Whether Apple has confirmed a terminal result for the bound digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotaryOutcome {
	Unknown,
	Pending,
	AcceptedWithVerifiedDigest,
	RejectedWithVerifiedDigest,
}

#[cfg(feature = "notarization-native")]
impl TryFrom<&crate::notarization::Report> for NotaryEvidence {
	type Error = super::ReleaseError;

	fn try_from(report: &crate::notarization::Report) -> Result<Self, Self::Error> {
		use crate::notarization::Status;

		let outcome = match (report.status, report.digest_verified) {
			(Status::Unknown, false) => NotaryOutcome::Unknown,
			(Status::Pending, false) => NotaryOutcome::Pending,
			(Status::Accepted, true) => NotaryOutcome::AcceptedWithVerifiedDigest,
			(Status::Rejected, true) => NotaryOutcome::RejectedWithVerifiedDigest,
			_ => return Err(Self::Error::MismatchedReceipt),
		};
		let image = ImageIdentity::new(
			report
				.artifact_sha256
				.as_deref()
				.ok_or(Self::Error::MismatchedReceipt)?,
			report
				.artifact_bytes
				.ok_or(Self::Error::MismatchedReceipt)?,
		)?;
		Ok(Self {
			backend: report.backend,
			image,
			submission_id: report.submission_id.as_ref().map(ToString::to_string),
			outcome,
		})
	}
}
