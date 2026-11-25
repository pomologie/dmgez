use std::{
	ffi::OsStr,
	fs,
	io::{Seek, SeekFrom, Write},
};

use dmgez_udif::{embedded_code_directory, read_embedded_signature, signed_signature_range};
use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};
use sha2::{Digest, Sha256};

use super::*;
use crate::native::{END_OPTIONS, runner};

const UDRO: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../../../tests/fixtures/images/tiny-UDRO.dmg"
));
const UDZO: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../../../tests/fixtures/images/tiny-UDZO.dmg"
));
const SOURCE_NAME: &str = "Unsigned.dmg";
const OUTPUT_NAME: &str = "Published.dmg";
const IDENTIFIER: &str = "org.dmgez.release-fixture";
const SHA256_OPTION: &str = "--digest-algorithm=sha256";
const SHA384_OPTION: &str = "--digest-algorithm=sha384";
const QUARANTINE: &str = "com.apple.quarantine";
const QUARANTINE_VALUE: &[u8] = b"0083;00000000;dmgez-test;";
const FIXTURE_FINGERPRINT: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn publishes_only_verified_synthetic_udro_and_udzo_appends() {
	for original in [UDRO, UDZO] {
		let directory = tempfile::tempdir().unwrap();
		let source = directory.path().join(SOURCE_NAME);
		let output = directory.path().join(OUTPUT_NAME);
		fs::write(&source, original).unwrap();
		let file = fs::File::open(&source).unwrap();
		fsetxattr(&file, QUARANTINE, QUARANTINE_VALUE, XattrFlags::empty()).unwrap();

		let artifact = open_unsigned(&source).unwrap();
		let result = stage_and_sign(&artifact, &output, sign_fixture, verify_fixture).unwrap();
		let bytes = fs::read(&output).unwrap();
		assert_eq!(result.path, output.canonicalize().unwrap());
		assert_eq!(result.unsigned_sha256, digest(original));
		assert_eq!(result.signed_sha256, digest(&bytes));
		assert_ne!(result.signed_sha256, result.unsigned_sha256);
		assert_eq!(fs::read(&source).unwrap(), original);
		assert!(matches!(
			open_unsigned(&output),
			Err(SignError::ExpectedUnsigned)
		));
		verify_fixture(&Artifact::open(&output).unwrap()).unwrap();
		assert_quarantine(&file);
		assert_quarantine(&fs::File::open(&output).unwrap());
		assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
	}
}

#[test]
fn withholds_output_when_signing_changes_the_original_body() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UDZO).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	let error = stage_and_sign(
		&artifact,
		&output,
		|path| {
			sign_fixture(path)?;
			let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
			file.write_all(&[UDZO[0] ^ 1]).unwrap();
			Ok(())
		},
		|_| panic!("body comparison must run before signature verification"),
	);
	assert!(matches!(error, Err(SignError::ImageChanged)));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), UDZO);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn withholds_output_when_the_appended_code_directory_has_the_wrong_body_digest() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UDZO).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	let result = stage_and_sign(
		&artifact,
		&output,
		|path| {
			sign_fixture(path)?;
			let mut file = fs::OpenOptions::new()
				.read(true)
				.write(true)
				.open(path)
				.unwrap();
			let trailer = image::read_trailer(&file)?;
			let range = signed_signature_range(&trailer, file.metadata().unwrap().len()).unwrap();
			file.seek(SeekFrom::Start(range.start)).unwrap();
			let frame = read_embedded_signature(&mut file, range.end - range.start).unwrap();
			let directory = embedded_code_directory(&frame).unwrap();
			let offset = directory.body_hash.as_ptr() as usize - frame.as_ptr() as usize;
			file.seek(SeekFrom::Start(range.start + offset as u64))
				.unwrap();
			file.write_all(&[frame[offset] ^ 1]).unwrap();
			Ok(())
		},
		|_| panic!("the independent CodeDirectory check must reject the altered frame"),
	);
	assert!(matches!(result, Err(SignError::ImageIntegrity(_))));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), UDZO);
}

#[test]
fn keeps_native_verification_for_other_code_directory_profiles() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UDZO).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	stage_and_sign(
		&artifact,
		&output,
		|path| sign_fixture_with_digest(path, SHA384_OPTION),
		verify_fixture,
	)
	.unwrap();
	assert!(output.is_file());
	assert_eq!(fs::read(&source).unwrap(), UDZO);
}

#[test]
fn refuses_invalid_native_checksum_before_invoking_the_signer() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	let mut bad_image = UDZO.to_vec();
	bad_image[0] ^= 1;
	fs::write(&source, &bad_image).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	let result = stage_and_sign(
		&artifact,
		&output,
		|_| panic!("the signer must not run after a failed checksum"),
		verify_fixture,
	);
	assert!(matches!(result, Err(SignError::ImageChecksum)));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), bad_image);
}

#[test]
fn fails_release_requirement_for_an_ad_hoc_signature() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UDZO).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	let developer = DeveloperId::new(FIXTURE_FINGERPRINT, IDENTIFIER).unwrap();
	let result = stage_and_sign(&artifact, &output, sign_fixture, |signed| {
		developer.verify(signed)
	});
	assert!(matches!(result, Err(SignError::SigningPolicy)));
	assert!(!output.exists());
	assert_eq!(fs::read(&source).unwrap(), UDZO);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn reserves_existing_destination_before_invoking_a_signer() {
	let directory = tempfile::tempdir().unwrap();
	let source = directory.path().join(SOURCE_NAME);
	let output = directory.path().join(OUTPUT_NAME);
	fs::write(&source, UDZO).unwrap();
	fs::write(&output, UDRO).unwrap();
	let artifact = open_unsigned(&source).unwrap();
	let result = stage_and_sign(
		&artifact,
		&output,
		|_| panic!("the signer must not run for an existing output"),
		verify_fixture,
	);
	assert!(matches!(result, Err(SignError::Stage(_))));
	assert_eq!(fs::read(&source).unwrap(), UDZO);
	assert_eq!(fs::read(&output).unwrap(), UDRO);
}

fn sign_fixture(path: &Path) -> Result<(), SignError> {
	sign_fixture_with_digest(path, SHA256_OPTION)
}

fn sign_fixture_with_digest(path: &Path, digest_option: &str) -> Result<(), SignError> {
	let options = [
		"--sign",
		"-",
		"--timestamp=none",
		digest_option,
		"--identifier",
		IDENTIFIER,
		END_OPTIONS,
	];
	let mut arguments: Vec<_> = options.iter().map(OsStr::new).collect();
	arguments.push(path.as_os_str());
	let result = runner::codesign(&arguments).unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	Ok(())
}

fn verify_fixture(image: &Artifact) -> Result<(), SignError> {
	let report = crate::native::check(image.path(), Operation::Verify, Backend::Native)?;
	assert_eq!(report.status, Status::Verified, "{:?}", report.diagnostics);
	Ok(())
}

fn assert_quarantine(file: &fs::File) {
	let mut buffer = [0; QUARANTINE_VALUE.len()];
	let len = fgetxattr(file, QUARANTINE, &mut buffer[..]).unwrap();
	assert_eq!(&buffer[..len], QUARANTINE_VALUE);
}

fn digest(bytes: &[u8]) -> String {
	Sha256::digest(bytes)
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect()
}
