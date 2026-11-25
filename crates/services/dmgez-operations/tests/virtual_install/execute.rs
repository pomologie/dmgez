use dmgez_operations::{
	extract::host::{HostError, StableDestination},
	install::{
		CopyLimits, CopySummary, InstallError, InstallFailure, execute_copy,
		execute_copy_with_check, prepare_copy,
	},
};
use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};

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

#[test]
fn checks_a_complete_private_bundle_before_publishing_it() {
	let root = tempfile::tempdir().unwrap();
	let path = root.path().canonicalize().unwrap();
	let target = path.join(APP_NAME);
	checked_copy(&path, |private_bundle| {
		assert_eq!(private_bundle.file_name().unwrap(), APP_NAME);
		assert!(private_bundle.starts_with(&path));
		assert_ne!(private_bundle, target);
		assert!(!target.exists());
		assert_eq!(fs::read(program(private_bundle))?, PROGRAM_BYTES);
		Ok(())
	})
	.unwrap();
	assert_eq!(fs::read(program(&target)).unwrap(), PROGRAM_BYTES);
	assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
}

#[test]
fn retains_the_private_bundle_on_requested_check_refusal() {
	let root = tempfile::tempdir().unwrap();
	let path = root.path().canonicalize().unwrap();
	let failed = checked_copy(&path, |_| Err(io::ErrorKind::PermissionDenied.into())).unwrap_err();
	assert!(
		matches!(failed.failure(), InstallFailure::Prepublication(error)
		if error.kind() == io::ErrorKind::PermissionDenied)
	);
	let retained = assert_retained_stage(&path, &failed);
	assert_eq!(fs::read(program(&retained)).unwrap(), PROGRAM_BYTES);
}

#[test]
fn checks_data_modes_and_extra_names_again_after_the_policy_hook() {
	type Mutation = fn(&Path) -> io::Result<()>;
	let mutations: [Mutation; 4] = [
		|app| fs::write(program(app), vec![0; PROGRAM_BYTES.len()]),
		|app| fs::set_permissions(program(app), fs::Permissions::from_mode(0o700)),
		|app| fs::write(app.join("unexpected"), b"unplanned content"),
		|app| {
			rustix::fs::setxattr(
				program(app),
				FIXTURE_ATTRIBUTE,
				b"altered attribute bytes",
				rustix::fs::XattrFlags::REPLACE,
			)
			.map_err(Into::into)
		},
	];
	for mutate in mutations {
		let root = tempfile::tempdir().unwrap();
		let path = root.path().canonicalize().unwrap();
		let failed = checked_copy(&path, mutate).unwrap_err();
		assert!(
			matches!(failed.failure(), InstallFailure::ReadBack),
			"{failed:?}"
		);
		assert_retained_stage(&path, &failed);
	}
}

fn checked_copy(
	path: &Path,
	check: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<CopySummary, InstallError<io::Error>> {
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
	let destination = StableDestination::open(path).unwrap();
	execute_copy_with_check(
		&mut view,
		&plan,
		&destination.directory(),
		|| Ok(()),
		|| false,
		check,
	)
}

fn program(bundle: &Path) -> std::path::PathBuf {
	bundle
		.join(CONTENTS_NAME)
		.join(MACOS_NAME)
		.join(EXECUTABLE_NAME)
}

fn assert_retained_stage(path: &Path, failed: &InstallError<io::Error>) -> std::path::PathBuf {
	assert!(!failed.published());
	assert!(!path.join(APP_NAME).exists());
	let retained = path
		.join(failed.residual_name().unwrap().as_str())
		.join(APP_NAME);
	assert!(retained.is_dir());
	retained
}
