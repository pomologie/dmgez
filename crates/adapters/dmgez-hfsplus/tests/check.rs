#![cfg(feature = "verify")]

use std::{error::Error as _, io};

use dmgez_hfsplus::{CheckError, CheckFailureKind, check};
use hfsplus_format::volume::{HEADER_BYTES, PRIMARY_HEADER_OFFSET};
use hfsplus_reader::check::{CheckError as StructureError, CheckLimit};

const WINDOW_BYTES: u64 = 4096;

#[test]
fn reports_an_unknown_signature_as_unsupported() {
	let error = check(WINDOW_BYTES, |offset, bytes| {
		assert_eq!(offset, PRIMARY_HEADER_OFFSET);
		assert_eq!(bytes.len(), HEADER_BYTES);
		bytes.fill(0);
		Ok::<(), io::Error>(())
	})
	.unwrap_err();
	assert_eq!(error.kind(), CheckFailureKind::Unsupported);
}

#[test]
fn preserves_the_source_read_failure() {
	let error = check(WINDOW_BYTES, |_, _| {
		Err(io::Error::from(io::ErrorKind::PermissionDenied))
	})
	.unwrap_err();
	assert_eq!(error.kind(), CheckFailureKind::Read);
	let underlying = error.source().unwrap().source().unwrap();
	assert_eq!(
		underlying.downcast_ref::<io::Error>().unwrap().kind(),
		io::ErrorKind::PermissionDenied,
	);
}

#[test]
fn keeps_a_structural_work_limit_distinct_from_corruption() {
	let error =
		CheckError::<io::Error>::Structure(StructureError::Limit(CheckLimit::CatalogEntries));
	assert_eq!(error.kind(), CheckFailureKind::ResourceLimit);
}
