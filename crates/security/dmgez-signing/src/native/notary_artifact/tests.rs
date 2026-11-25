use std::{ffi::OsStr, fs};

use super::*;
use crate::native::{END_OPTIONS, runner};

const IMAGE_NAME: &str = "fixture.dmg";
const IMAGE_BYTES: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../../../tests/fixtures/images/tiny-UDZO.dmg"
));
const IMAGE_DIGEST: &str = "40701c4d6d5bc1c60f83f7ea7c5e3b9d28a7ca1c9d08758fd2f7bbab175fd6d5";
const AD_HOC_SIGN_OPTIONS: &[&str] = &[
	"--sign",
	"-",
	"--timestamp=none",
	"--identifier",
	"org.dmgez.notarization-fixture",
	END_OPTIONS,
];

#[test]
fn binds_the_exact_bytes_of_a_private_copy() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join(IMAGE_NAME);
	fs::write(&path, IMAGE_BYTES).unwrap();
	let stage = PreparedArtifact::copy(&open_image(&path).unwrap()).unwrap();
	assert_ne!(stage.path(), path);
	assert_eq!(stage.path().file_name(), path.file_name());
	assert_eq!(stage.len(), IMAGE_BYTES.len() as u64);
	assert_eq!(stage.sha256(), IMAGE_DIGEST);
	assert_eq!(fs::read(stage.path()).unwrap(), IMAGE_BYTES);
	assert!(fs::metadata(stage.path()).unwrap().permissions().readonly());
	stage.validate().unwrap();
	let staged = stage.path().to_path_buf();
	drop(stage);
	assert!(!staged.exists());
	assert_eq!(fs::read(&path).unwrap(), IMAGE_BYTES);
}

#[test]
fn revalidation_rejects_a_replaced_staging_entry_even_with_identical_bytes() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join(IMAGE_NAME);
	fs::write(&path, IMAGE_BYTES).unwrap();
	let stage = PreparedArtifact::copy(&open_image(&path).unwrap()).unwrap();
	fs::remove_file(stage.path()).unwrap();
	fs::write(stage.path(), IMAGE_BYTES).unwrap();
	assert!(matches!(stage.validate(), Err(Error::Changed)));
	assert_eq!(fs::read(&path).unwrap(), IMAGE_BYTES);
}

#[test]
fn refuses_unsigned_and_ad_hoc_signed_images() {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join(IMAGE_NAME);
	fs::write(&path, IMAGE_BYTES).unwrap();
	assert!(matches!(PreparedArtifact::new(&path), Err(Error::Unsigned)));

	let path = path.canonicalize().unwrap();
	let mut arguments: Vec<_> = AD_HOC_SIGN_OPTIONS.iter().map(OsStr::new).collect();
	arguments.push(path.as_os_str());
	assert!(runner::codesign(&arguments).unwrap().status.success());
	let ad_hoc_bytes = fs::read(&path).unwrap();
	assert!(matches!(
		PreparedArtifact::new(&path),
		Err(Error::Signature(SignError::Timestamp))
	));
	assert_eq!(fs::read(&path).unwrap(), ad_hoc_bytes);
}
