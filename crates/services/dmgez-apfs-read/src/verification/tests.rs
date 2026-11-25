use std::{error::Error as _, io};

use crate::{CheckData, CheckFailureKind, check, probe};

const SOURCE_LENGTH: u64 = 4096;

#[test]
fn cancellation_before_opening_does_not_read_the_source() {
	let error = check(
		SOURCE_LENGTH,
		|_, _| -> io::Result<()> { panic!("cancelled verification read the source") },
		None,
		None,
		CheckData::Metadata,
		|| true,
	)
	.unwrap_err();
	assert_eq!(error.kind(), CheckFailureKind::Cancelled);
}

#[test]
fn source_failures_stay_distinct_from_malformed_filesystem_data() {
	fn unreadable(_: u64, _: &mut [u8]) -> io::Result<()> {
		Err(io::Error::other("fixture source failed"))
	}

	let check_error = check(
		SOURCE_LENGTH,
		unreadable,
		None,
		None,
		CheckData::Metadata,
		|| false,
	)
	.unwrap_err();
	let probe_error = probe(SOURCE_LENGTH, unreadable).unwrap_err();
	assert_eq!(check_error.kind(), CheckFailureKind::Read);
	assert_eq!(probe_error.kind(), CheckFailureKind::Read);
	assert_eq!(
		check_error
			.source()
			.and_then(std::error::Error::source)
			.and_then(|source| source.downcast_ref::<io::Error>())
			.map(io::Error::kind),
		Some(io::ErrorKind::Other),
	);
	assert_eq!(
		probe_error
			.source()
			.and_then(|source| source.downcast_ref::<io::Error>())
			.map(io::Error::kind),
		Some(io::ErrorKind::Other),
	);
}
