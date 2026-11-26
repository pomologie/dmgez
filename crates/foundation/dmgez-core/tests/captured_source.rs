#![cfg(feature = "source")]

use std::io::{self, Read as _, Write as _};

use dmgez_core::source::{CapturedFile, SourceError};

const CHUNK_BYTES: usize = 32 * 1024;
const TAIL: &[u8] = b"final bytes";

#[test]
fn sequential_readers_copy_exactly_the_admitted_extent() {
	let mut file = tempfile::tempfile().unwrap();
	let mut payload = vec![0x42; CHUNK_BYTES];
	payload.extend_from_slice(TAIL);
	file.write_all(&payload).unwrap();
	let source = CapturedFile::new(file).unwrap();
	let mut first = source.reader();
	let mut second = source.reader();

	let mut first_copy = vec![0; CHUNK_BYTES];
	first.read_exact(&mut first_copy).unwrap();
	let mut second_copy = Vec::new();
	assert_eq!(
		io::copy(&mut second, &mut second_copy).unwrap(),
		source.len(),
	);
	assert_eq!(
		io::copy(&mut first, &mut first_copy).unwrap(),
		TAIL.len() as u64
	);
	assert_eq!(io::copy(&mut first, &mut first_copy).unwrap(), 0);
	assert_eq!(first_copy, payload);
	assert_eq!(second_copy, payload);
}

#[test]
fn truncation_reports_changed_source_instead_of_successful_eof() {
	let mut file = tempfile::tempfile().unwrap();
	file.write_all(TAIL).unwrap();
	let source = CapturedFile::new(file.try_clone().unwrap()).unwrap();
	let mut reader = source.reader();
	let mut prefix = [0];
	reader.read_exact(&mut prefix).unwrap();
	assert_eq!(prefix, [TAIL[0]]);
	file.set_len(1).unwrap();

	let mut rest = Vec::new();
	let error = io::copy(&mut reader, &mut rest).unwrap_err();
	assert!(matches!(
		error.get_ref().and_then(|cause| cause.downcast_ref()),
		Some(SourceError::Changed),
	));
	assert!(rest.is_empty());
}
