use std::collections::VecDeque;

use super::*;

const ID: &str = "7642bf96-13a1-4906-aa65-49b3dfbb15f1";
const UNRELATED_ID: &str = "40e5f5f4-6f24-4bac-8635-fda2e94d55a3";
const SHA256: &str = "b869a03d3af158c1972fca0afba2aebc229d01ab273c32ca7ecf3d7f5a43db22";
const OTHER_SHA256: &str = "a05fc80b580c50261e58f7a8048929b6e54f36ca7eb84bf801b0566348685d26";
const RECORD: &str = "submission.json";
const IMAGE: &str = "Fixture.dmg";

struct Mock<'a> {
	record: Option<&'a Path>,
	receipt: Option<Receipt>,
	upload: Option<Result<Submission, CallError>>,
	statuses: VecDeque<Status>,
	submits: usize,
}

impl Service for Mock<'_> {
	fn submit(&mut self, _: &Path) -> Result<Submission, CallError> {
		self.submits += 1;
		let (_, reserved) = Store::open(self.record.unwrap()).unwrap();
		assert_eq!(reserved.status, Status::Unknown);
		assert!(reserved.submission_id.is_none());
		self.upload.take().expect("upload must run only once")
	}

	fn status(&mut self, id: &Id) -> Result<Submission, CallError> {
		Ok(Submission {
			id: id.clone(),
			status: self.statuses.pop_front().expect("unexpected status call"),
		})
	}

	fn log(&mut self, _: &Id) -> Result<Receipt, CallError> {
		self.receipt.take().ok_or(CallError::Failed)
	}
}

fn reserve() -> Report {
	Report::reserve(SHA256.to_owned(), 4_096, 0)
}

fn mock(record: &Path, upload: Result<Submission, CallError>) -> Mock<'_> {
	Mock {
		record: Some(record),
		receipt: None,
		upload: Some(upload),
		statuses: VecDeque::new(),
		submits: 0,
	}
}

fn receipt(sha256: &str) -> Receipt {
	Receipt {
		id: ID.parse().unwrap(),
		sha256: sha256.to_owned(),
		status: Status::Accepted,
		summary: LogSummary {
			errors: 0,
			warnings: 1,
		},
	}
}

fn begin(
	service: &mut Mock<'_>,
	record: &Path,
	pause: impl FnMut() -> Pause,
) -> Result<Report, Error> {
	submit_with(
		service,
		Path::new(IMAGE),
		record,
		reserve(),
		|| Ok(()),
		pause,
	)
}

#[test]
fn ambiguous_upload_is_reserved_and_recovered_only_by_matching_digest() {
	let temp = tempfile::tempdir().unwrap();
	let record = temp.path().join(RECORD);
	let mut service = mock(&record, Err(CallError::Failed));

	let report = begin(&mut service, &record, || Pause::Disabled).unwrap();
	assert_eq!(report.status, Status::Unknown);
	assert_eq!(report.issue, Some(Issue::UploadUnconfirmed));
	assert!(begin(&mut service, &record, || Pause::Disabled).is_err());
	assert_eq!(service.submits, 1);

	service.statuses.push_back(Status::Pending);
	let pending = status_with(&mut service, ID.parse().unwrap(), Some(&record), || {
		Pause::Disabled
	})
	.unwrap();
	assert_eq!(pending.issue, Some(Issue::DigestUnconfirmed));
	assert!(pending.submission_id.is_none());

	service.statuses.push_back(Status::Accepted);
	service.receipt = Some(receipt(OTHER_SHA256));
	let mismatch = status_with(&mut service, ID.parse().unwrap(), Some(&record), || {
		Pause::Disabled
	})
	.unwrap();
	assert_eq!(mismatch.issue, Some(Issue::ReceiptMismatch));
	assert!(mismatch.submission_id.is_none());
	assert!(!mismatch.digest_verified);

	service.statuses.push_back(Status::Accepted);
	service.receipt = Some(receipt(SHA256));
	let recovered = status_with(&mut service, ID.parse().unwrap(), Some(&record), || {
		Pause::Disabled
	})
	.unwrap();
	assert_eq!(recovered.status, Status::Accepted);
	assert!(recovered.digest_verified);
	assert_eq!(recovered.submission_id, Some(ID.parse().unwrap()));
	assert_eq!(service.submits, 1);
}

#[test]
fn wait_persists_id_and_polls_without_resubmitting() {
	let temp = tempfile::tempdir().unwrap();
	let record = temp.path().join(RECORD);
	let mut service = mock(
		&record,
		Ok(Submission {
			id: ID.parse().unwrap(),
			status: Status::Pending,
		}),
	);
	service.statuses.extend([Status::Pending, Status::Accepted]);
	service.receipt = Some(receipt(SHA256));
	let report = begin(&mut service, &record, || {
		let (_, on_disk) = Store::open(&record).unwrap();
		assert_eq!(on_disk.submission_id, Some(ID.parse().unwrap()));
		assert_eq!(on_disk.status, Status::Pending);
		Pause::Continue
	})
	.unwrap();
	assert_eq!(report.status, Status::Accepted);
	assert!(report.digest_verified);
	assert_eq!(service.submits, 1);
	assert_eq!(Store::open(&record).unwrap().1, report);
}

#[test]
fn expired_wait_and_unrelated_id_do_not_upload_or_overwrite_unknown_evidence() {
	let temp = tempfile::tempdir().unwrap();
	let record = temp.path().join(RECORD);
	let mut service = mock(
		&record,
		Ok(Submission {
			id: ID.parse().unwrap(),
			status: Status::Pending,
		}),
	);
	let report = begin(&mut service, &record, || Pause::Expired).unwrap();
	assert_eq!(report.status, Status::Pending);
	assert_eq!(report.issue, Some(Issue::WaitExpired));
	assert!(matches!(
		status_with(
			&mut service,
			UNRELATED_ID.parse().unwrap(),
			Some(&record),
			|| Pause::Disabled
		),
		Err(Error::UnrelatedId)
	));
	assert_eq!(Store::open(&record).unwrap().1, report);
	assert_eq!(service.submits, 1);
}

#[test]
fn rejection_is_preserved_without_logging_private_wire_details() {
	let temp = tempfile::tempdir().unwrap();
	let record = temp.path().join(RECORD);
	let mut service = mock(
		&record,
		Ok(Submission {
			id: ID.parse().unwrap(),
			status: Status::Rejected,
		}),
	);
	service.receipt = Some(Receipt {
		status: Status::Rejected,
		..receipt(SHA256)
	});
	let result = begin(&mut service, &record, || Pause::Disabled).unwrap();
	assert_eq!(result.status, Status::Rejected);
	assert!(result.digest_verified);
	assert_eq!(
		result.log,
		Some(LogSummary {
			errors: 0,
			warnings: 1
		})
	);
}
