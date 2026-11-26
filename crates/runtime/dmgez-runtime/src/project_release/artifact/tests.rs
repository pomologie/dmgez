use dmgez_core::output::stage_new;
use dmgez_udif::{
	SECTOR_BYTES,
	writer::{Profile, WriteOptions, write_image},
};

use super::*;

const SOURCE_NAME: &str = "source.dmg";
const OUTPUT_NAME: &str = "release.dmg";
const STAGED_SUFFIX: &str = ".dmg";
const UNSIGNED: &[u8] = include_bytes!("../../../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNED: &[u8] =
	include_bytes!("../../../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const SIGNED_SHA256: &str = "5b2bd2040ed7234affd8db97f05717a7541f821470d1451d26699f4ec1f8395e";

#[test]
fn publishes_the_exact_signed_image_once() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, SIGNED).unwrap();
	let expected = ImageIdentity::new(SIGNED_SHA256, SIGNED.len() as u64).unwrap();
	assert_eq!(observe(&source).unwrap(), expected);
	assert_eq!(verify(&source).unwrap(), expected);
	publish(&source, &expected, &output).unwrap();
	assert_eq!(fs::read(&output).unwrap(), SIGNED);
	assert!(publish(&source, &expected, &output).is_err());
	assert_eq!(fs::read(&output).unwrap(), SIGNED);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn rejects_a_changed_source_length_before_staging() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, SIGNED).unwrap();
	let admitted = verify(&source).unwrap();
	let shortened = &SIGNED[..SIGNED.len() - 1];
	fs::write(&source, shortened).unwrap();

	let error = publish(&source, &admitted, &output).unwrap_err();
	assert_eq!(error.kind(), io::ErrorKind::InvalidData);
	assert_eq!(error.to_string(), IMAGE_IDENTITY_CHANGED);
	assert_eq!(fs::read(&source).unwrap(), shortened);
	assert_eq!(
		fs::metadata(&output).unwrap_err().kind(),
		io::ErrorKind::NotFound
	);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn image_identity_requires_bounded_nonempty_input_and_an_exact_read() {
	let over_limit = Limits::default().max_physical_bytes + 1;
	for bytes in [0, over_limit] {
		assert_eq!(
			hash_image(io::empty(), bytes).unwrap_err().kind(),
			io::ErrorKind::InvalidData,
		);
	}
	assert_eq!(
		hash_image(io::empty(), 1).unwrap_err().kind(),
		io::ErrorKind::UnexpectedEof,
	);
}

#[test]
fn image_identity_hashes_exactly_the_admitted_bytes_across_chunks() {
	const FINAL_CHUNK_BYTES: usize = 3;
	const UNADMITTED_BYTES: usize = 1;
	let admitted = HASH_BUFFER_BYTES + FINAL_CHUNK_BYTES;
	let data = vec![0x42; admitted + UNADMITTED_BYTES];
	let mut reader = data.as_slice();

	let identity = hash_image(&mut reader, admitted as u64).unwrap();
	let digest: String = Sha256::digest(&data[..admitted])
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect();
	assert_eq!(identity.sha256(), digest);
	assert_eq!(identity.bytes(), admitted as u64);
	assert_eq!(reader, &data[admitted..]);
}

#[test]
fn entry_comparison_rejects_length_and_timestamp_changes_on_the_same_handle() {
	use std::time::{Duration, SystemTime};

	const TIME_STEP_SECONDS: u64 = 60;
	const GROWN_FILE_BYTES: u64 = 1;
	let file = tempfile::tempfile().unwrap();
	let initial = file.metadata().unwrap();
	file.set_len(GROWN_FILE_BYTES).unwrap();
	let grown = file.metadata().unwrap();
	assert_ne!(initial.len(), grown.len());
	assert!(!same_entry(&initial, &grown));

	file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(TIME_STEP_SECONDS))
		.unwrap();
	let retimed = file.metadata().unwrap();
	assert_eq!(grown.len(), retimed.len());
	assert_ne!(grown.modified().unwrap(), retimed.modified().unwrap());
	assert!(!same_entry(&grown, &retimed));
}

#[cfg(unix)]
#[test]
fn entry_comparison_detects_a_different_inode_at_the_same_length() {
	let first_file = tempfile::tempfile().unwrap();
	let second_file = tempfile::tempfile().unwrap();
	let first = first_file.metadata().unwrap();
	let second = second_file.metadata().unwrap();
	assert_eq!(first.len(), second.len());
	assert!(!same_entry(&first, &second));
}

#[test]
fn stale_identity_and_corrupt_payload_leave_no_output_or_staging_entry() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UNSIGNED).unwrap();
	let expected = verify(&source).unwrap();
	let mut corrupt = UNSIGNED.to_vec();
	corrupt[0] ^= 1;
	fs::write(&source, corrupt).unwrap();
	assert!(publish(&source, &expected, &output).is_err());
	assert!(publish(&source, &observe(&source).unwrap(), &output).is_err());
	assert!(!output.exists());
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn stage_recheck_requires_the_writer_checksum_values() {
	let directory = tempfile::tempdir().unwrap();
	let output = directory.path().join(OUTPUT_NAME);
	let options = WriteOptions {
		profile: Profile::Udro,
		..WriteOptions::default()
	};
	let payload = [0x53; SECTOR_BYTES as usize];
	let mut stage = stage_new(
		&output,
		STAGED_SUFFIX,
		|writer| write_image(payload.as_slice(), SECTOR_BYTES, writer, &options).map_err(invalid),
		verify_stage,
	)
	.unwrap();
	let mut report = stage.report().clone();
	report.master_crc ^= 1;
	assert!(verify_stage(stage.reader().unwrap(), &report).is_err());
	assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn rejects_a_repointable_symbolic_link_as_the_release_source() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let link = directory.path().join("linked.dmg");
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UNSIGNED).unwrap();
	std::os::unix::fs::symlink(&source, &link).unwrap();
	assert!(observe(&link).is_err());
	assert!(verify(&link).is_err());
	assert!(publish(&link, &observe(&source).unwrap(), &output).is_err());
	assert!(!output.exists());
}
