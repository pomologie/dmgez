use std::{
	path::Path,
	thread,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::native::{notary_artifact::PreparedArtifact, notary_tool::Tool};

use super::{CallError, Error, Id, Issue, LogSummary, Report, Status, store::Store, valid_digest};

#[cfg(test)]
mod tests;

const WAIT_LIMIT: Duration = Duration::from_secs(20 * 60);
const INITIAL_DELAY: Duration = Duration::from_secs(2);
const MAX_DELAY: Duration = Duration::from_secs(30);

enum Pause {
	Continue,
	Disabled,
	Expired,
}

pub(crate) trait Service {
	fn submit(&mut self, artifact: &Path) -> Result<Submission, CallError>;
	fn status(&mut self, id: &Id) -> Result<Submission, CallError>;
	fn log(&mut self, id: &Id) -> Result<Receipt, CallError>;
}

pub(crate) struct Submission {
	pub id: Id,
	pub status: Status,
}

pub(crate) struct Receipt {
	pub id: Id,
	pub sha256: String,
	pub status: Status,
	pub summary: LogSummary,
}

pub(super) fn submit(
	image: &Path,
	record: &Path,
	profile: &str,
	wait: bool,
) -> Result<Report, Error> {
	let mut service = Tool::new(profile).map_err(tool_error)?;
	let artifact = PreparedArtifact::new(image).map_err(artifact_error)?;
	let reservation = Report::reserve(artifact.sha256().to_owned(), artifact.len(), now()?);
	submit_with(
		&mut service,
		artifact.path(),
		record,
		reservation,
		|| artifact.validate().map_err(artifact_error),
		pause_for(wait),
	)
}

pub(super) fn status(
	id: Id,
	record: Option<&Path>,
	profile: &str,
	wait: bool,
) -> Result<Report, Error> {
	let mut service = Tool::new(profile).map_err(tool_error)?;
	status_with(&mut service, id, record, pause_for(wait))
}

fn submit_with(
	service: &mut impl Service,
	artifact: &Path,
	record: &Path,
	mut report: Report,
	validate: impl Fn() -> Result<(), Error>,
	pause: impl FnMut() -> Pause,
) -> Result<Report, Error> {
	validate()?;
	// Persist the intention before calling a tool that can create a remote job.
	// The reservation survives a crash, timeout, invalid response, or rejection.
	let mut store = Store::create(record, &report).map_err(record_error)?;
	validate()?;
	let response = match service.submit(artifact) {
		Ok(response) => response,
		Err(_) => return Ok(report),
	};
	let id = response.id;
	report.submission_id = Some(id.clone());
	save(&mut store, &mut report, &id)?;
	validate()?;
	poll(
		service,
		id,
		Some(&mut store),
		report,
		Some(response.status),
		pause,
	)
}

fn status_with(
	service: &mut impl Service,
	id: Id,
	record: Option<&Path>,
	pause: impl FnMut() -> Pause,
) -> Result<Report, Error> {
	if let Some(path) = record {
		let (mut store, report) = Store::open(path).map_err(record_error)?;
		if report
			.submission_id
			.as_ref()
			.is_some_and(|known| known != &id)
		{
			return Err(Error::UnrelatedId);
		}
		poll(service, id, Some(&mut store), report, None, pause)
	} else {
		let mut report = Report::unbound(now()?);
		report.submission_id = Some(id.clone());
		poll(service, id, None, report, None, pause)
	}
}

fn poll(
	service: &mut impl Service,
	id: Id,
	mut store: Option<&mut Store>,
	mut report: Report,
	mut initial: Option<Status>,
	mut pause: impl FnMut() -> Pause,
) -> Result<Report, Error> {
	loop {
		let remote = initial
			.take()
			.map_or_else(|| fetch_status(service, &id), Ok);
		report.status = Status::Unknown;
		report.digest_verified = false;
		report.log = None;
		report.issue = None;
		match remote {
			Ok(status) if status.terminal() => read_receipt(service, &id, status, &mut report),
			Ok(Status::Pending) if report.submission_id.is_some() => {
				report.status = Status::Pending
			}
			Ok(Status::Pending) => report.issue = Some(Issue::DigestUnconfirmed),
			Ok(_) => report.issue = Some(Issue::MalformedResponse),
			Err(error) => report.issue = Some(issue(error)),
		}
		if let Some(store) = store.as_deref_mut() {
			save(store, &mut report, &id)?;
		} else {
			report.updated_at = now()?.max(report.updated_at);
		}
		if remote != Ok(Status::Pending) {
			return Ok(report);
		}
		match pause() {
			Pause::Continue => {}
			Pause::Disabled => return Ok(report),
			Pause::Expired => {
				report.issue = Some(Issue::WaitExpired);
				if let Some(store) = store {
					save(store, &mut report, &id)?;
				}
				return Ok(report);
			}
		}
	}
}

fn fetch_status(service: &mut impl Service, id: &Id) -> Result<Status, CallError> {
	let result = service.status(id)?;
	if &result.id == id {
		Ok(result.status)
	} else {
		Err(CallError::Malformed)
	}
}

fn read_receipt(service: &mut impl Service, id: &Id, status: Status, report: &mut Report) {
	let receipt = match service.log(id) {
		Ok(receipt) => receipt,
		Err(error) => {
			report.issue = Some(issue(error));
			return;
		}
	};
	if &receipt.id != id
		|| receipt.status != status
		|| !valid_digest(&receipt.sha256)
		|| report
			.artifact_sha256
			.as_deref()
			.is_some_and(|digest| !digest.eq_ignore_ascii_case(&receipt.sha256))
	{
		report.issue = Some(Issue::ReceiptMismatch);
		return;
	}
	report.digest_verified = report.artifact_bytes.is_some();
	report.artifact_sha256 = Some(receipt.sha256.to_ascii_lowercase());
	report.submission_id = Some(receipt.id);
	report.status = status;
	report.log = Some(receipt.summary);
}

fn save(store: &mut Store, report: &mut Report, id: &Id) -> Result<(), Error> {
	report.updated_at = now()?.max(report.updated_at);
	store
		.replace(report)
		.map_err(|source| Error::SaveSubmission {
			id: id.clone(),
			source: Box::new(source),
		})
}

fn pause_for(wait: bool) -> impl FnMut() -> Pause {
	let mut start = None;
	let mut delay = INITIAL_DELAY;
	move || {
		if !wait {
			return Pause::Disabled;
		}
		let start = start.get_or_insert_with(Instant::now);
		let remaining = WAIT_LIMIT.saturating_sub(start.elapsed());
		if remaining.is_zero() {
			return Pause::Expired;
		}
		thread::sleep(delay.min(remaining));
		delay = (delay * 2).min(MAX_DELAY);
		if start.elapsed() < WAIT_LIMIT {
			Pause::Continue
		} else {
			Pause::Expired
		}
	}
}

fn now() -> Result<u64, Error> {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|time| time.as_secs())
		.map_err(|_| Error::Clock)
}

fn issue(error: CallError) -> Issue {
	match error {
		CallError::Unavailable => Issue::BackendUnavailable,
		CallError::Malformed => Issue::MalformedResponse,
		_ => Issue::RequestFailed,
	}
}

fn tool_error(error: CallError) -> Error {
	Error::Tool(Box::new(error))
}

fn artifact_error(error: crate::native::notary_artifact::Error) -> Error {
	Error::Artifact(Box::new(error))
}

fn record_error(error: super::store::Error) -> Error {
	Error::Record(Box::new(error))
}
