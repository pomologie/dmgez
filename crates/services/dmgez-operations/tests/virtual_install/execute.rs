use dmgez_operations::{
	extract::host::{HostError, StableDestination},
	install::{CopyLimits, InstallFailure, execute_copy, prepare_copy},
};
use std::{fs, os::unix::fs::PermissionsExt};

#[cfg(target_os = "macos")]
use super::planning::RESOURCE_BYTES;
use super::{
	planning::{LINK_TARGET, selection},
	*,
};

#[test]
fn publishes_only_after_independent_data_and_metadata_readback() {
	let reads = Cell::new(0);
	let mut view = Fixture {
		link: Some(LINK_TARGET),
		attribute: Some(b"exact attribute bytes"),
		#[cfg(target_os = "macos")]
		resource: Some(RESOURCE_BYTES),
		..Fixture::new(&reads)
	}
	.view();
	let app = selection(&mut view);
	let plan = prepare_copy(&mut view, app, CopyLimits::default(), || false).unwrap();
	let root = tempfile::tempdir().unwrap();
	let path = root.path().canonicalize().unwrap();
	let target = path.join(APP_NAME);
	let destination = StableDestination::open(&path).unwrap();
	let verified = execute_copy(
		&mut view,
		&plan,
		&destination.directory(),
		|| {
			assert!(!target.exists());
			Ok(())
		},
		|| false,
	)
	.unwrap();
	assert_eq!(verified, plan.summary());
	assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
	let contents = target.join(CONTENTS_NAME);
	let program = contents.join(MACOS_NAME).join(EXECUTABLE_NAME);
	assert_eq!(fs::read(contents.join(INFO_NAME)).unwrap(), INFO);
	assert_eq!(fs::read(&program).unwrap(), PROGRAM_BYTES);
	assert_eq!(
		fs::metadata(&program).unwrap().permissions().mode() as u16 & 0o777,
		EXECUTABLE_MODE
	);
	assert_eq!(
		fs::metadata(&target).unwrap().permissions().mode() as u16 & 0o777,
		DIRECTORY_MODE
	);
	assert_eq!(
		fs::read_link(contents.join("Library"))
			.unwrap()
			.as_os_str()
			.as_encoded_bytes(),
		LINK_TARGET
	);
	let mut value = [0; 64];
	let bytes = rustix::fs::getxattr(&program, FIXTURE_ATTRIBUTE, value.as_mut_slice()).unwrap();
	assert_eq!(&value[..bytes], b"exact attribute bytes");
	#[cfg(target_os = "macos")]
	{
		let bytes =
			rustix::fs::getxattr(&program, "com.apple.ResourceFork", value.as_mut_slice()).unwrap();
		assert_eq!(&value[..bytes], RESOURCE_BYTES);
	}
	assert!(reads.get() >= 6);
}

#[test]
fn preserves_a_late_incumbent_and_identifies_the_retained_stage() {
	let reads = Cell::new(0);
	let mut view = Fixture {
		link: Some(LINK_TARGET),
		..Fixture::new(&reads)
	}
	.view();
	let app = selection(&mut view);
	let plan = prepare_copy(&mut view, app, CopyLimits::default(), || false).unwrap();
	let root = tempfile::tempdir().unwrap();
	let path = root.path().canonicalize().unwrap();
	let target = path.join(APP_NAME);
	let destination = StableDestination::open(&path).unwrap();
	let failed = execute_copy(
		&mut view,
		&plan,
		&destination.directory(),
		|| {
			if reads.get() >= 5 && !target.exists() {
				fs::write(&target, b"incumbent")?;
			}
			Ok(())
		},
		|| false,
	)
	.unwrap_err();
	assert!(matches!(
		failed.failure(),
		InstallFailure::Host(HostError::Conflict)
	));
	assert!(!failed.published());
	assert_eq!(fs::read(target).unwrap(), b"incumbent");
	let residual = failed.residual_name().unwrap();
	assert!(
		path.join(residual.as_str())
			.join(APP_NAME)
			.join(CONTENTS_NAME)
			.is_dir()
	);
}
