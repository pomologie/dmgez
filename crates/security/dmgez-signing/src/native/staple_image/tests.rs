use std::{cell::Cell, fs};

use dmgez_udif::TRAILER_BYTES;
use sha2::{Digest, Sha256};

use crate::native::disk::image::encode_hex;

use super::*;

const UNSIGNED_FIXTURE: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../../../tests/fixtures/images/tiny-UDZO.dmg"
));
const ID: &str = "7642bf96-13a1-4906-aa65-49b3dfbb15f1";
const SOURCE: &str = "Accepted.dmg";
const OUTPUT: &str = "Stapled.dmg";
const SIGNATURE_OFFSET_FIELD: usize = 296;
const SIGNATURE_LENGTH_FIELD: usize = 304;
const SUPERBLOB: u32 = 0xfade_0cc0;
const DIRECTORY_BLOB: u32 = 0xfade_0c02;
const TICKET_BLOB: u32 = 0xfade_0b01;
const DIRECTORY_SLOT: u32 = 0;
const TICKET_SLOT: u32 = 0x0001_0002;
const SUPERBLOB_HEADER: usize = 12;
const INDEX_ROW: usize = 8;

#[test]
fn validates_exact_staged_bytes_before_publishing_a_ticket_copy() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let output = directory.path().join(OUTPUT);
	let input = fixture(false);
	let stapled = fixture(true);
	fs::write(&source, &input).unwrap();
	let checks = Cell::new(0);
	let report = transact(
		&open_image(&source).unwrap(),
		&output,
		accepted(&input),
		|_| {
			checks.set(checks.get() + 1);
			Ok(())
		},
		|path| {
			assert_eq!(fs::read(path)?, input);
			fs::write(path, &stapled)?;
			Ok(())
		},
		|path| {
			assert_eq!(fs::read(path)?, stapled);
			Ok(())
		},
	)
	.unwrap();
	assert_eq!(checks.get(), 2);
	assert_eq!(report.submission_id.to_string(), ID);
	assert_eq!(report.input_sha256, digest(&input));
	assert_eq!(report.output_sha256, digest(&stapled));
	assert!(report.ticket_validated);
	assert_eq!(fs::read(&output).unwrap(), stapled);
	assert_eq!(fs::read(&source).unwrap(), input);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn refuses_a_different_same_size_input_before_ticket_work() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let output = directory.path().join(OUTPUT);
	let input = fixture(false);
	let expected = accepted(&input);
	let mut changed = input;
	changed[0] ^= 1;
	fs::write(&source, &changed).unwrap();
	let result = transact(
		&open_image(&source).unwrap(),
		&output,
		expected,
		|_| panic!("unexpected native image check"),
		|_| panic!("unexpected ticket request"),
		|_| panic!("unexpected ticket check"),
	);
	assert!(matches!(result, Err(Error::DigestMismatch)));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), changed);
}

#[test]
fn withholds_output_if_the_stapling_operation_changes_image_bytes() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let output = directory.path().join(OUTPUT);
	let input = fixture(false);
	fs::write(&source, &input).unwrap();
	let mut stapled = fixture(true);
	stapled[0] ^= 1;
	let result = transact(
		&open_image(&source).unwrap(),
		&output,
		accepted(&input),
		|_| Ok(()),
		|path| Ok(fs::write(path, &stapled)?),
		|_| panic!("unexpected ticket check after a body change"),
	);
	assert!(matches!(result, Err(Error::Changed)));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), input);
}

#[test]
fn withholds_output_if_native_ticket_validation_fails() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE);
	let output = directory.path().join(OUTPUT);
	let input = fixture(false);
	fs::write(&source, &input).unwrap();
	let result = transact(
		&open_image(&source).unwrap(),
		&output,
		accepted(&input),
		|_| Ok(()),
		|path| Ok(fs::write(path, fixture(true))?),
		|_| Err(io::Error::other("synthetic native validation failure").into()),
	);
	assert!(matches!(result, Err(Error::Io(_))));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), input);
}

fn accepted(bytes: &[u8]) -> AcceptedImage {
	AcceptedImage {
		id: ID.parse().unwrap(),
		digest: digest(bytes),
		bytes: bytes.len() as u64,
	}
}

fn digest(bytes: &[u8]) -> String {
	encode_hex(&Sha256::digest(bytes))
}

// Only the local format comparison is under test. These short components are
// deliberately synthetic and are never handed to Apple's ticket tools.
fn fixture(with_ticket: bool) -> Vec<u8> {
	let original_trailer = UNSIGNED_FIXTURE.len() - TRAILER_BYTES;
	let (body, trailer) = UNSIGNED_FIXTURE.split_at(original_trailer);
	let directory = component(DIRECTORY_BLOB, b"synthetic code directory");
	let ticket = component(TICKET_BLOB, b"synthetic ticket");
	let count = if with_ticket { 2 } else { 1 };
	let directory_offset = SUPERBLOB_HEADER + count * INDEX_ROW;
	let ticket_offset = directory_offset + directory.len();
	let length = ticket_offset + if with_ticket { ticket.len() } else { 0 };
	let mut bytes = body.to_vec();
	for field in [
		SUPERBLOB,
		length as u32,
		count as u32,
		DIRECTORY_SLOT,
		directory_offset as u32,
	] {
		bytes.extend(field.to_be_bytes());
	}
	if with_ticket {
		bytes.extend(TICKET_SLOT.to_be_bytes());
		bytes.extend((ticket_offset as u32).to_be_bytes());
	}
	bytes.extend(directory);
	if with_ticket {
		bytes.extend(ticket);
	}
	let mut trailer = trailer.to_vec();
	trailer[SIGNATURE_OFFSET_FIELD..][..8].copy_from_slice(&(body.len() as u64).to_be_bytes());
	trailer[SIGNATURE_LENGTH_FIELD..][..8].copy_from_slice(&(length as u64).to_be_bytes());
	bytes.extend(trailer);
	bytes
}

fn component(magic: u32, body: &[u8]) -> Vec<u8> {
	let mut bytes = Vec::new();
	bytes.extend(magic.to_be_bytes());
	bytes.extend(((2 * size_of::<u32>() + body.len()) as u32).to_be_bytes());
	bytes.extend_from_slice(body);
	bytes
}
