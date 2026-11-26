use std::{
	fs,
	io::{Seek, Write},
	path::PathBuf,
};

use dmgez_udif::embedded_code_directory;

use super::{stage_and_sign, verify_signed};
use crate::SignError;

const UNSIGNED: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNED: &[u8] =
	include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const UNSIGNED_SHA256: &str = "40701c4d6d5bc1c60f83f7ea7c5e3b9d28a7ca1c9d08758fd2f7bbab175fd6d5";
const SIGNED_SHA256: &str = "5b2bd2040ed7234affd8db97f05717a7541f821470d1451d26699f4ec1f8395e";
const INPUT_NAME: &str = "installer.dmg";
const OUTPUT_NAME: &str = "installer-signed.dmg";
const INCUMBENT: &[u8] = b"existing output";

struct Fixture {
	directory: tempfile::TempDir,
	source: PathBuf,
	destination: PathBuf,
}

impl Fixture {
	fn new(bytes: &[u8]) -> Self {
		let directory = tempfile::tempdir().unwrap();
		let source = directory.path().join(INPUT_NAME);
		let destination = directory.path().join(OUTPUT_NAME);
		fs::write(&source, bytes).unwrap();
		Self {
			directory,
			source,
			destination,
		}
	}

	fn entry_count(&self) -> usize {
		fs::read_dir(self.directory.path()).unwrap().count()
	}
}

fn replace_stage(stage: &mut fs::File, bytes: &[u8]) -> Result<(), SignError> {
	stage.rewind().map_err(SignError::ImageIo)?;
	stage.write_all(bytes).map_err(SignError::ImageIo)?;
	stage
		.set_len(bytes.len() as u64)
		.map_err(SignError::ImageIo)
}

fn copy_native_signature(stage: &mut fs::File) -> Result<(), SignError> {
	replace_stage(stage, SIGNED)
}

#[test]
fn publishes_native_append_fixture_with_verified_digests() {
	let fixture = Fixture::new(UNSIGNED);
	let report = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		copy_native_signature,
		|frame| {
			assert!(embedded_code_directory(frame).is_ok());
			Ok(())
		},
	)
	.unwrap();
	assert_eq!(report.path, fixture.destination.canonicalize().unwrap());
	assert_eq!(report.source, fixture.source.canonicalize().unwrap());
	assert_eq!(report.unsigned_sha256, UNSIGNED_SHA256);
	assert_eq!(report.signed_sha256, SIGNED_SHA256);
	assert_eq!(fs::read(&fixture.source).unwrap(), UNSIGNED);
	assert_eq!(fs::read(&fixture.destination).unwrap(), SIGNED);
	assert_eq!(fixture.entry_count(), 2);
	verify_signed(&fixture.destination, |_| Ok(())).unwrap();
	assert!(matches!(
		verify_signed(&fixture.source, |_| panic!("unsigned image has no frame")),
		Err(SignError::Unsigned)
	));
}

#[test]
fn refuses_bad_unsigned_checksums_before_invoking_signer() {
	let mut corrupt = UNSIGNED.to_vec();
	corrupt[0] ^= 1;
	let fixture = Fixture::new(&corrupt);
	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		|_| panic!("checksum failure must stop before signing"),
		|_| Ok(()),
	);
	assert!(matches!(result, Err(SignError::ImageIntegrity(_))));
	assert_eq!(fs::read(&fixture.source).unwrap(), corrupt);
	assert_eq!(fixture.entry_count(), 1);
}

#[test]
fn rejects_signed_input_without_touching_the_destination() {
	let fixture = Fixture::new(SIGNED);
	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		|_| panic!("already signed"),
		|_| Ok(()),
	);
	assert!(matches!(result, Err(SignError::ExpectedUnsigned)));
	assert_eq!(fs::read(&fixture.source).unwrap(), SIGNED);
	assert_eq!(fixture.entry_count(), 1);
}

#[test]
fn rejects_same_size_body_tampering_and_policy_refusal() {
	let fixture = Fixture::new(UNSIGNED);
	let mut corrupt = SIGNED.to_vec();
	corrupt[0] ^= 1;
	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		|stage| replace_stage(stage, &corrupt),
		|_| panic!("body mismatch must precede policy"),
	);
	assert!(matches!(result, Err(SignError::ImageChanged)));
	assert_eq!(fixture.entry_count(), 1);

	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		copy_native_signature,
		|_| Err(SignError::SigningPolicy),
	);
	assert!(matches!(result, Err(SignError::SigningPolicy)));
	assert_eq!(fs::read(&fixture.source).unwrap(), UNSIGNED);
	assert_eq!(fixture.entry_count(), 1);
}

#[test]
fn signed_verification_checks_the_body_before_invoking_policy() {
	const FIRST_PAYLOAD_BYTE: usize = 0;
	let mut changed = SIGNED.to_vec();
	changed[FIRST_PAYLOAD_BYTE] ^= 1;
	let fixture = Fixture::new(&changed);
	let result = verify_signed(&fixture.source, |_| {
		panic!("signature policy must not receive an image with a mismatched body digest")
	});
	assert!(matches!(result, Err(SignError::ImageIntegrity(_))));
	assert_eq!(fs::read(&fixture.source).unwrap(), changed);
	assert_eq!(fixture.entry_count(), 1);
}

#[cfg(unix)]
#[test]
fn signed_verification_rechecks_held_metadata_after_policy() {
	use std::time::Duration;

	const TIMESTAMP_CHANGE: Duration = Duration::from_secs(60);
	let fixture = Fixture::new(SIGNED);
	let source = fs::OpenOptions::new()
		.write(true)
		.open(&fixture.source)
		.unwrap();
	let original = source.metadata().unwrap().modified().unwrap();
	let changed = original.checked_sub(TIMESTAMP_CHANGE).unwrap();
	let result = verify_signed(&fixture.source, |frame| {
		assert!(embedded_code_directory(frame).is_ok());
		source.set_modified(changed).map_err(SignError::ImageIo)
	});
	assert!(matches!(result, Err(SignError::ImageChanged)));
	assert_ne!(source.metadata().unwrap().modified().unwrap(), original);
	assert_eq!(fs::read(&fixture.source).unwrap(), SIGNED);
	assert_eq!(fixture.entry_count(), 1);
}

#[cfg(unix)]
#[test]
fn signed_verification_rechecks_the_name_after_policy() {
	const IMAGE_DIRECTORY: &str = "images";
	const MOVED_DIRECTORY: &str = "moved";
	let fixture = Fixture::new(SIGNED);
	let directory = fixture.directory.path().canonicalize().unwrap();
	let parent = directory.join(IMAGE_DIRECTORY);
	let moved_parent = directory.join(MOVED_DIRECTORY);
	let input = parent.join(INPUT_NAME);
	fs::create_dir(&parent).unwrap();
	fs::rename(&fixture.source, &input).unwrap();
	let result = verify_signed(&input, |frame| {
		assert!(embedded_code_directory(frame).is_ok());
		fs::rename(&parent, &moved_parent).map_err(SignError::ImageIo)?;
		fs::create_dir(&parent).map_err(SignError::ImageIo)?;
		fs::write(&input, SIGNED).map_err(SignError::ImageIo)
	});
	assert!(matches!(
		result,
		Err(SignError::Signature(crate::Error::ArtifactChanged { path })) if path == input
	));
	assert_eq!(fs::read(moved_parent.join(INPUT_NAME)).unwrap(), SIGNED);
	assert_eq!(fs::read(input).unwrap(), SIGNED);
}

#[test]
fn preserves_early_and_late_output_incumbents() {
	let fixture = Fixture::new(UNSIGNED);
	fs::write(&fixture.destination, INCUMBENT).unwrap();
	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		|_| panic!("early collision must stop signing"),
		|_| Ok(()),
	);
	assert!(matches!(result, Err(SignError::Stage(_))));
	assert_eq!(fs::read(&fixture.destination).unwrap(), INCUMBENT);
	fs::remove_file(&fixture.destination).unwrap();

	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		copy_native_signature,
		|_| fs::write(&fixture.destination, INCUMBENT).map_err(SignError::ImageIo),
	);
	assert!(matches!(result, Err(SignError::Stage(_))));
	assert_eq!(fs::read(&fixture.destination).unwrap(), INCUMBENT);
	assert_eq!(fs::read(&fixture.source).unwrap(), UNSIGNED);
	assert_eq!(fixture.entry_count(), 2);
}

#[cfg(unix)]
#[test]
fn refuses_final_symlink_inputs_and_detects_replaced_source_names() {
	use std::os::unix::fs::symlink;

	let fixture = Fixture::new(UNSIGNED);
	let link = fixture.directory.path().join("symlink.dmg");
	symlink(&fixture.source, &link).unwrap();
	let result = stage_and_sign(
		&link,
		&fixture.destination,
		|_| panic!("must not follow link"),
		|_| Ok(()),
	);
	assert!(result.is_err());
	fs::remove_file(&link).unwrap();

	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		copy_native_signature,
		|_| {
			fs::rename(&fixture.source, &link).map_err(SignError::ImageIo)?;
			fs::write(&fixture.source, UNSIGNED).map_err(SignError::ImageIo)
		},
	);
	assert!(matches!(
		result,
		Err(SignError::ImageChanged | SignError::Signature(_))
	));
	assert!(!fixture.destination.exists());
	assert_eq!(fixture.entry_count(), 2);
}

#[cfg(target_os = "macos")]
#[test]
fn preserves_the_exact_source_quarantine_attribute() {
	use rustix::fs::{XattrFlags, fsetxattr};
	const QUARANTINE: &str = "com.apple.quarantine";
	const VALUE: &[u8] = b"0083;00000000;dmgez-test;";

	let fixture = Fixture::new(UNSIGNED);
	fsetxattr(
		fs::File::open(&fixture.source).unwrap(),
		QUARANTINE,
		VALUE,
		XattrFlags::empty(),
	)
	.unwrap();
	stage_and_sign(
		&fixture.source,
		&fixture.destination,
		copy_native_signature,
		|_| Ok(()),
	)
	.unwrap();
	for path in [&fixture.source, &fixture.destination] {
		let file = fs::File::open(path).unwrap();
		assert_eq!(
			super::quarantine::read(&file, path).unwrap().as_deref(),
			Some(VALUE)
		);
	}
}

#[cfg(windows)]
#[test]
fn preserves_the_exact_zone_identifier_or_refuses_before_signing() {
	const VALUE: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

	let fixture = Fixture::new(UNSIGNED);
	let witness = fixture.directory.path().join("unrelated-source");
	let renamed = fixture.directory.path().join("unrelated-rename");
	fs::write(&witness, b"synthetic").unwrap();
	let probe = fs::File::open(&witness).unwrap();
	super::quarantine::write(&probe, &witness, Some(VALUE)).unwrap();
	drop(probe);
	fs::rename(&witness, &renamed).unwrap();
	let preserves_stream = super::quarantine::read(&fs::File::open(&renamed).unwrap(), &renamed)
		.unwrap()
		.as_deref()
		== Some(VALUE);
	super::quarantine::write(
		&fs::File::open(&fixture.source).unwrap(),
		&fixture.source,
		Some(VALUE),
	)
	.unwrap();
	let result = stage_and_sign(
		&fixture.source,
		&fixture.destination,
		|stage| {
			assert!(
				preserves_stream,
				"unsupported metadata must stop before signing"
			);
			copy_native_signature(stage)
		},
		|_| Ok(()),
	);
	let check_marker = |path: &std::path::Path| {
		let file = fs::File::open(path).unwrap();
		assert_eq!(
			super::quarantine::read(&file, path).unwrap().as_deref(),
			Some(VALUE)
		);
	};
	check_marker(&fixture.source);
	if preserves_stream {
		result.unwrap();
		check_marker(&fixture.destination);
	} else {
		assert!(
			matches!(result, Err(SignError::Quarantine(error)) if error.kind() == std::io::ErrorKind::Unsupported)
		);
		assert!(!fixture.destination.exists());
	}
}
