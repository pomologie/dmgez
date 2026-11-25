//! Native Apple notarytool adapter. Only typed results leave this module.

use std::{ffi::OsStr, io::ErrorKind, path::Path, time::Duration};

use serde::Deserialize;

use super::runner;
use crate::notarization::{
	CallError, Id, LogSummary, Receipt, Service, Status, Submission, valid_digest,
};

#[cfg(test)]
mod tests;

const VERSION: &str = "--version";
const SUBMIT: &str = "submit";
const INFO: &str = "info";
const LOG: &str = "log";
const NO_WAIT: &str = "--no-wait";
const NO_PROGRESS: &str = "--no-progress";
const JSON: &str = "--output-format=json";
const KEYCHAIN_PROFILE: &str = "--keychain-profile=";
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const RESPONSE_LIMIT: usize = 256 * 1024;
const LOG_LIMIT: usize = 4 * 1024 * 1024;
const LOG_FORMAT_VERSION: u8 = 1;
const ACCEPTED: &str = "Accepted";
const IN_PROGRESS: &str = "In Progress";
const INVALID: &str = "Invalid";
const REJECTED: &str = "Rejected";
const ERROR: &str = "error";
const WARNING: &str = "warning";

/// Invocation authorized to use an existing Keychain profile.
pub(crate) struct Tool {
	profile_arg: String,
}

/// Detect access rules that can override owner-only POSIX record permissions.
pub(crate) fn has_acl(path: &Path) -> Result<bool, CallError> {
	if !path.is_absolute() {
		return Err(CallError::Malformed);
	}
	let output = runner::list_acl(path).map_err(|_| CallError::Failed)?;
	if !output.status.success() {
		return Err(CallError::Failed);
	}
	let line = output
		.stdout
		.strip_suffix(b"\n")
		.ok_or(CallError::Malformed)?;
	if !line.starts_with(b"-") && !line.starts_with(b"d") {
		return Err(CallError::Malformed);
	}
	// -b escapes line breaks in names, -n makes ownership numeric, and -e
	// prints each ACL entry on its own line even when xattrs hide the + marker.
	Ok(line.contains(&b'\n'))
}

impl Tool {
	pub(crate) fn new(profile: &str) -> Result<Self, CallError> {
		let profile_arg = profile_argument(profile)?;
		// Version discovery is local and never passes the profile.
		let output = request(&[OsStr::new(VERSION)], VERSION_TIMEOUT, RESPONSE_LIMIT)
			.map_err(|_| CallError::Unavailable)?;
		if output.is_empty() {
			return Err(CallError::Unavailable);
		}
		Ok(Self { profile_arg })
	}
}

impl Service for Tool {
	fn submit(&mut self, artifact: &Path) -> Result<Submission, CallError> {
		let args = [
			OsStr::new(SUBMIT),
			OsStr::new(NO_WAIT),
			OsStr::new(NO_PROGRESS),
			OsStr::new(JSON),
			OsStr::new(&self.profile_arg),
			artifact.as_os_str(),
		];
		let output = request(&args, UPLOAD_TIMEOUT, RESPONSE_LIMIT)?;
		read_submission(&output, Response::Upload)
	}

	fn status(&mut self, id: &Id) -> Result<Submission, CallError> {
		let id = id.to_string();
		let args = [
			OsStr::new(INFO),
			OsStr::new(JSON),
			OsStr::new(NO_PROGRESS),
			OsStr::new(&self.profile_arg),
			OsStr::new(&id),
		];
		let output = request(&args, QUERY_TIMEOUT, RESPONSE_LIMIT)?;
		read_submission(&output, Response::Query)
	}

	fn log(&mut self, id: &Id) -> Result<Receipt, CallError> {
		let id = id.to_string();
		let args = [
			OsStr::new(LOG),
			OsStr::new(NO_PROGRESS),
			OsStr::new(&self.profile_arg),
			OsStr::new(&id),
		];
		let output = request(&args, QUERY_TIMEOUT, LOG_LIMIT)?;
		read_receipt(&output)
	}
}

fn profile_argument(profile: &str) -> Result<String, CallError> {
	if profile.trim().is_empty() || profile.chars().any(char::is_control) {
		return Err(CallError::InvalidProfile);
	}
	Ok(format!("{KEYCHAIN_PROFILE}{profile}"))
}

fn request(args: &[&OsStr], timeout: Duration, limit: usize) -> Result<Vec<u8>, CallError> {
	let output = runner::notarytool(args, timeout, limit).map_err(|error| match error {
		runner::Error::Spawn(source) if source.kind() == ErrorKind::NotFound => {
			CallError::Unavailable
		}
		_ => CallError::Failed,
	})?;
	if !output.status.success() {
		return Err(CallError::Failed);
	}
	Ok(output.stdout)
}

#[derive(Deserialize)]
struct WireSubmission {
	id: Id,
	status: Option<String>,
}

#[derive(Clone, Copy)]
enum Response {
	Upload,
	Query,
}

fn read_submission(json: &[u8], response: Response) -> Result<Submission, CallError> {
	let wire: WireSubmission = serde_json::from_slice(json).map_err(|_| CallError::Malformed)?;
	let status = match wire.status {
		Some(status) => map_status(&status),
		// The successful no-wait upload response reports the ID without a status.
		None if matches!(response, Response::Upload) => Status::Pending,
		None => return Err(CallError::Malformed),
	};
	Ok(Submission {
		id: wire.id,
		status,
	})
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReceipt {
	log_format_version: u8,
	job_id: Id,
	sha256: String,
	status: String,
	issues: Option<Vec<WireIssue>>,
}

#[derive(Deserialize)]
struct WireIssue {
	severity: String,
}

fn read_receipt(json: &[u8]) -> Result<Receipt, CallError> {
	let wire: WireReceipt = serde_json::from_slice(json).map_err(|_| CallError::Malformed)?;
	let status = map_status(&wire.status);
	if wire.log_format_version != LOG_FORMAT_VERSION
		|| !valid_digest(&wire.sha256)
		|| !matches!(status, Status::Accepted | Status::Rejected)
	{
		return Err(CallError::Malformed);
	}
	let mut summary = LogSummary {
		errors: 0,
		warnings: 0,
	};
	// The native runner bounds total bytes, so these counts fit in u32.
	for issue in wire.issues.into_iter().flatten() {
		match issue.severity.as_str() {
			ERROR => summary.errors += 1,
			WARNING => summary.warnings += 1,
			_ => {}
		}
	}
	Ok(Receipt {
		id: wire.job_id,
		sha256: wire.sha256.to_ascii_lowercase(),
		status,
		summary,
	})
}

fn map_status(status: &str) -> Status {
	match status {
		ACCEPTED => Status::Accepted,
		IN_PROGRESS => Status::Pending,
		INVALID | REJECTED => Status::Rejected,
		_ => Status::Unknown,
	}
}
