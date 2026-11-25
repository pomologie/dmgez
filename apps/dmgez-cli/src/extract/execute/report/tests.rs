use std::{ffi::OsStr, io, os::unix::ffi::OsStrExt, path::Path};

use dmgez_runtime::extraction::Progress;
use serde_json::json;

use super::Report;

#[test]
fn preserves_exact_destination_bytes_when_a_parent_is_not_utf8() {
	let destination = b"/parent-\xff/output";
	let path = Path::new(OsStr::from_bytes(destination));
	let report = serde_json::to_value(Report::new("/source", path)).unwrap();
	assert_eq!(report["destination_bytes"], json!(destination.as_slice()));
}

#[test]
fn reports_possible_partial_output_after_unconfirmed_directory_creation() {
	let mut report = Report::new("/source", Path::new("/output"));
	let mut progress = Progress::default();
	progress.directory_creation_unconfirmed = true;
	report.progress = Some(progress);
	report.fail(&io::Error::other("directory attachment failed"));
	let report = serde_json::to_value(report).unwrap();
	assert_eq!(report["status"], "partial");
	assert_eq!(report["progress"]["directories_created"], 0);
	assert_eq!(report["progress"]["directory_creation_unconfirmed"], true);
}
