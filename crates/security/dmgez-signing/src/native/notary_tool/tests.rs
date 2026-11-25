use super::*;

const ID: &str = "6ae6d671-409b-4da3-8eb4-7f73356137ca";
const DIGEST: &str = "6ec2f92cc6d710ea2fcbd84591949d1535fec61089b3faa2729e6f31ea026a85";

#[test]
fn successful_upload_can_report_the_id_without_a_status() {
	let json = format!(r#"{{"id":"{ID}","message":"Successfully uploaded file"}}"#);
	let result = read_submission(json.as_bytes(), Response::Upload).unwrap();
	assert_eq!(result.id.to_string(), ID);
	assert_eq!(result.status, Status::Pending);
	assert!(matches!(
		read_submission(json.as_bytes(), Response::Query),
		Err(CallError::Malformed)
	));
}

#[test]
fn validates_response_identifiers_and_distinguishes_service_states() {
	for (wire, expected) in [
		(ACCEPTED, Status::Accepted),
		(IN_PROGRESS, Status::Pending),
		(INVALID, Status::Rejected),
		(REJECTED, Status::Rejected),
		("new status", Status::Unknown),
	] {
		let json = format!(r#"{{"id":"{ID}","status":"{wire}"}}"#);
		let result = read_submission(json.as_bytes(), Response::Query).unwrap();
		assert_eq!(result.status, expected);
	}
	assert!(matches!(
		read_submission(
			br#"{"id":"not a UUID","status":"Accepted"}"#,
			Response::Query
		),
		Err(CallError::Malformed)
	));
}

#[test]
fn reduces_private_log_issues_to_counts() {
	let json = format!(
		r#"{{"logFormatVersion":1,"jobId":"{ID}","sha256":"{}","status":"Invalid","issues":[
			{{"severity":"error","message":"private text"}},
			{{"severity":"warning","path":"private path"}},
			{{"severity":"info"}}
		]}}"#,
		DIGEST.to_ascii_uppercase(),
	);
	let receipt = read_receipt(json.as_bytes()).unwrap();
	assert_eq!(receipt.id.to_string(), ID);
	assert_eq!(receipt.status, Status::Rejected);
	assert_eq!(receipt.sha256, DIGEST);
	assert_eq!(receipt.summary.errors, 1);
	assert_eq!(receipt.summary.warnings, 1);
	let json = format!(
		r#"{{"logFormatVersion":1,"jobId":"{ID}","sha256":"{DIGEST}","status":"Accepted","issues":null}}"#
	);
	let receipt = read_receipt(json.as_bytes()).unwrap();
	assert_eq!(receipt.status, Status::Accepted);
	assert_eq!(receipt.summary.errors, 0);
	assert_eq!(receipt.summary.warnings, 0);
	let json = format!(
		r#"{{"logFormatVersion":1,"jobId":"{ID}","sha256":"{DIGEST}","status":"Accepted"}}"#
	);
	let receipt = read_receipt(json.as_bytes()).unwrap();
	assert_eq!(receipt.summary.errors, 0);
	assert_eq!(receipt.summary.warnings, 0);
}

#[test]
fn rejects_receipt_without_valid_digest_or_terminal_status() {
	for (version, digest, status) in [
		(1, "malformed digest", ACCEPTED),
		(1, DIGEST, IN_PROGRESS),
		(2, DIGEST, ACCEPTED),
	] {
		let json = format!(
			r#"{{"logFormatVersion":{version},"jobId":"{ID}","sha256":"{digest}","status":"{status}"}}"#
		);
		assert!(matches!(
			read_receipt(json.as_bytes()),
			Err(CallError::Malformed)
		));
	}
}

#[test]
fn profile_is_a_single_opaque_argument_and_refuses_control_characters() {
	assert_eq!(
		profile_argument("--synthetic profile").unwrap(),
		"--keychain-profile=--synthetic profile",
	);
	for bad in ["", " \t ", "name\n", "\0"] {
		assert!(matches!(
			profile_argument(bad),
			Err(CallError::InvalidProfile)
		));
	}
}
