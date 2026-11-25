//! Explicit native notarization and digest-bound submission records.

use std::{fmt, path::Path, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ResolvedBackend;

#[cfg(target_os = "macos")]
mod flow;
mod staple;
#[cfg(target_os = "macos")]
mod store;
#[cfg(target_os = "macos")]
pub(crate) use flow::{Receipt, Service, Submission};
#[cfg(target_os = "macos")]
pub(crate) use staple::AcceptedImage;
pub use staple::{StapleReport, staple};

#[cfg(target_os = "macos")]
const REPORT_VERSION: u16 = 1;
const UUID_LENGTH: usize = 36;
const UUID_SEPARATORS: [usize; 4] = [8, 13, 18, 23];

/// Validated UUID assigned by the Apple notary service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String")]
pub struct Id(String);

impl FromStr for Id {
	type Err = Error;

	fn from_str(value: &str) -> Result<Self, Error> {
		if value.len() != UUID_LENGTH
			|| !value.bytes().enumerate().all(|(index, byte)| {
				if UUID_SEPARATORS.contains(&index) {
					byte == b'-'
				} else {
					byte.is_ascii_hexdigit()
				}
			}) {
			return Err(Error::InvalidId);
		}
		Ok(Self(value.to_ascii_lowercase()))
	}
}

impl TryFrom<String> for Id {
	type Error = Error;

	fn try_from(value: String) -> Result<Self, Error> {
		value.parse()
	}
}

impl fmt::Display for Id {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

/// Outcome confirmed by the available evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
	Unknown,
	Pending,
	Accepted,
	Rejected,
}

impl Status {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Unknown => "unknown",
			Self::Pending => "pending",
			Self::Accepted => "accepted",
			Self::Rejected => "rejected",
		}
	}

	#[cfg(target_os = "macos")]
	fn terminal(self) -> bool {
		matches!(self, Self::Accepted | Self::Rejected)
	}
}

/// Redacted explanation for an incomplete outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Issue {
	UploadUnconfirmed,
	BackendUnavailable,
	RequestFailed,
	MalformedResponse,
	DigestUnconfirmed,
	ReceiptMismatch,
	WaitExpired,
}

impl Issue {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::UploadUnconfirmed => "upload_unconfirmed",
			Self::BackendUnavailable => "backend_unavailable",
			Self::RequestFailed => "request_failed",
			Self::MalformedResponse => "malformed_response",
			Self::DigestUnconfirmed => "digest_unconfirmed",
			Self::ReceiptMismatch => "receipt_mismatch",
			Self::WaitExpired => "wait_expired",
		}
	}
}

/// Redacted counts from a verified Apple log. Raw issue messages are private.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogSummary {
	pub errors: u32,
	pub warnings: u32,
}

/// Notarization report and private local submission record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Report {
	pub schema_version: u16,
	pub backend: ResolvedBackend,
	pub submission_id: Option<Id>,
	pub status: Status,
	pub artifact_sha256: Option<String>,
	pub artifact_bytes: Option<u64>,
	/// Seconds since the Unix epoch.
	pub created_at: u64,
	pub updated_at: u64,
	/// The terminal Apple log was matched against the reserved local digest.
	pub digest_verified: bool,
	pub log: Option<LogSummary>,
	pub issue: Option<Issue>,
}

#[cfg(target_os = "macos")]
impl Report {
	fn unbound(created_at: u64) -> Self {
		Self {
			schema_version: REPORT_VERSION,
			backend: ResolvedBackend::Native,
			submission_id: None,
			status: Status::Unknown,
			artifact_sha256: None,
			artifact_bytes: None,
			created_at,
			updated_at: created_at,
			digest_verified: false,
			log: None,
			issue: None,
		}
	}

	fn reserve(sha256: String, bytes: u64, created_at: u64) -> Self {
		Self {
			artifact_sha256: Some(sha256),
			artifact_bytes: Some(bytes),
			issue: Some(Issue::UploadUnconfirmed),
			..Self::unbound(created_at)
		}
	}

	fn valid_record(&self) -> bool {
		self.schema_version == REPORT_VERSION
			&& self.updated_at >= self.created_at
			&& self.artifact_sha256.as_deref().is_some_and(valid_digest)
			&& self.artifact_bytes.is_some_and(|bytes| bytes > 0)
			&& (self.status == Status::Unknown || self.submission_id.is_some())
			&& self.digest_verified == self.status.terminal()
			&& self.log.is_some() == self.status.terminal()
	}
}

#[cfg(target_os = "macos")]
pub(crate) fn valid_digest(value: &str) -> bool {
	const SHA256_HEX_LENGTH: usize = 64;
	value.len() == SHA256_HEX_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum CallError {
	#[error("native notarization tool is unavailable")]
	Unavailable,
	#[error("invalid Keychain profile name")]
	InvalidProfile,
	#[error("the native notarization request failed")]
	Failed,
	#[error("the native notarization response was malformed")]
	Malformed,
}

/// Local or operational error from a notarization command.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("native notarization is unavailable on this platform")]
	Unavailable,
	#[error("the submission ID must be a UUID")]
	InvalidId,
	#[error("the submission ID does not match the local record")]
	UnrelatedId,
	#[error("the system clock precedes the Unix epoch")]
	Clock,
	#[error("could not prepare the signed disk image")]
	Artifact(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("could not open or save the notarization record")]
	Record(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("could not save the notarization record for submission {id}")]
	SaveSubmission {
		id: Id,
		#[source]
		source: Box<dyn std::error::Error + Send + Sync>,
	},
	#[error("could not prepare the native notarization tool: {0}")]
	Tool(#[source] Box<dyn std::error::Error + Send + Sync>),
	#[error("the record must contain an accepted notarization with a verified image digest")]
	AcceptanceRequired,
	#[error("could not staple and publish the disk image")]
	Staple(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Reserve a new private record, then submit an exact copy of the signed DMG.
///
/// An ambiguous upload leaves the reservation in place. The caller can query
/// status with a recovered Apple submission ID. A wait only polls that ID.
///
/// # Errors
/// Fails for unavailable tooling, an invalid artifact or profile, or a record
/// that could not be reserved or updated.
pub fn submit(image: &Path, record: &Path, profile: &str, wait: bool) -> Result<Report, Error> {
	#[cfg(target_os = "macos")]
	{
		flow::submit(image, record, profile, wait)
	}
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (image, record, profile, wait);
		Err(Error::Unavailable)
	}
}

/// Query an Apple submission, optionally updating a local record.
///
/// Terminal status for a local record requires a matching artifact digest in
/// the Apple log. An ID recovered after an ambiguous upload is bound only by
/// that evidence.
///
/// # Errors
/// Fails for an invalid ID, unavailable tooling, or an unusable record.
pub fn status(id: &str, record: Option<&Path>, profile: &str, wait: bool) -> Result<Report, Error> {
	let id = id.parse::<Id>()?;
	#[cfg(target_os = "macos")]
	{
		flow::status(id, record, profile, wait)
	}
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (id, record, profile, wait);
		Err(Error::Unavailable)
	}
}
