use std::{
	fs, io,
	os::unix::fs::{PermissionsExt, symlink},
};

use dmgez_vfs::volume::ForkKind;

use super::{
	ForkWriteError, HostError,
	fixture::{
		DATA, DESTINATION, DETACHED, LIMITS, NESTED, SENTINEL, Scratch, assert_children, host_name,
		with_fork,
	},
};
const ALL_PERMISSION_BITS: u32 = 0o7777;
const OWNER_READ_WRITE: u32 = 0o600;
const OWNER_EXECUTABLE: u32 = 0o700;
const PUBLIC_READABLE: u32 = 0o644;
const STAGE_OUTSIDE_NAME: &str = "moved-stage";

#[test]
fn distinguishes_post_creation_admission_failure_from_existing_directory() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(NESTED);
	let result = directory.create_child_with(&name, || {
		assert!(scratch.destination.join(NESTED).is_dir());
		Err(HostError::Inspect(io::ErrorKind::PermissionDenied.into()))
	});
	let Err(HostError::CreatedDirectoryUnavailable { source }) = result else {
		panic!("expected a post-creation error, got {result:?}");
	};
	assert!(
		matches!(*source, HostError::Inspect(error) if error.kind() == io::ErrorKind::PermissionDenied)
	);
	assert_children(&scratch.destination, &[NESTED]);
	assert!(matches!(
		directory.create_child_with(&name, || panic!("already exists")),
		Err(HostError::Conflict)
	));
	scratch.outside_untouched();
}

#[test]
fn dry_run_checks_absence_and_a_detached_root_without_writing() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(DESTINATION);
	let target = scratch.outside.join("dangling-target");
	directory.require_absent(&name).unwrap();
	assert_children(&scratch.destination, &[]);

	let alias = scratch.destination.join(DESTINATION);
	symlink(&target, &alias).unwrap();
	assert!(matches!(
		directory.require_absent(&name),
		Err(HostError::Conflict)
	));
	assert_eq!(fs::read_link(&alias).unwrap(), target);
	assert!(!target.exists());

	let detached = scratch.outside.join(DETACHED);
	fs::rename(&scratch.destination, &detached).unwrap();
	fs::create_dir(&scratch.destination).unwrap();
	assert!(matches!(
		directory.require_absent(&name),
		Err(HostError::Detached)
	));
	assert_children(&scratch.destination, &[]);
	assert_children(&detached, &[DESTINATION]);
	scratch.outside_untouched();
}

#[test]
fn stages_nonexecutable_bytes_and_publishes_only_owner_permissions() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(DESTINATION);
	let mut stage_mode = None;
	let written = with_fork(
		DATA,
		None,
		ForkKind::Data,
		|offset, _| {
			if offset == 0 {
				let staging = fs::read_dir(&scratch.destination)?.next().unwrap()?.path();
				let mode = fs::metadata(staging)?.permissions().mode() & ALL_PERMISSION_BITS;
				assert_eq!(mode & !OWNER_READ_WRITE, 0);
				stage_mode = Some(mode);
			}
			Ok(())
		},
		|fork| directory.publish_fork_verified(&name, fork, LIMITS, || false, || Ok(())),
	)
	.unwrap()
	.unwrap();
	let destination = scratch.destination.join(DESTINATION);
	assert_eq!(written, DATA.len() as u64);
	assert_eq!(fs::read(&destination).unwrap(), DATA);
	assert_eq!(
		fs::metadata(&destination).unwrap().permissions().mode() & ALL_PERMISSION_BITS,
		stage_mode.unwrap()
	);
	assert_children(&scratch.destination, &[DESTINATION]);
}

#[test]
fn final_source_identity_failure_leaves_no_publication_or_stage() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(DESTINATION);
	let result = with_fork(
		DATA,
		None,
		ForkKind::Data,
		|_, _| Ok(()),
		|fork| {
			directory.publish_fork_verified(
				&name,
				fork,
				LIMITS,
				|| false,
				|| {
					assert!(!scratch.destination.join(DESTINATION).exists());
					let staging = fs::read_dir(&scratch.destination)?.next().unwrap()?.path();
					assert_eq!(fs::read(staging)?, DATA);
					Err(io::ErrorKind::InvalidData.into())
				},
			)
		},
	)
	.unwrap();
	assert!(
		matches!(result, Err(ForkWriteError::SourceIdentity(error)) if error.kind() == io::ErrorKind::InvalidData)
	);
	assert_children(&scratch.destination, &[]);
	scratch.outside_untouched();
}

#[test]
fn rechecks_stage_identity_after_the_final_source_check() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(DESTINATION);
	let moved = scratch.outside.join(STAGE_OUTSIDE_NAME);
	let mut replacement = None;
	let result = with_fork(
		DATA,
		None,
		ForkKind::Data,
		|_, _| Ok(()),
		|fork| {
			directory.publish_fork_verified(
				&name,
				fork,
				LIMITS,
				|| false,
				|| {
					let stage = fs::read_dir(&scratch.destination)?.next().unwrap()?.path();
					fs::rename(&stage, &moved)?;
					fs::write(&stage, SENTINEL)?;
					replacement = Some(stage);
					Ok(())
				},
			)
		},
	)
	.unwrap();
	assert!(matches!(
		result,
		Err(ForkWriteError::Host(HostError::StageChanged))
	));
	assert_eq!(fs::read(replacement.unwrap()).unwrap(), SENTINEL);
	assert_eq!(fs::read(moved).unwrap(), DATA);
	assert!(!scratch.destination.join(DESTINATION).exists());
	scratch.outside_untouched();
}

#[test]
fn refuses_a_stage_that_gained_execute_or_public_permissions() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let name = host_name(DESTINATION);
	for mode in [OWNER_EXECUTABLE, PUBLIC_READABLE] {
		let result = with_fork(
			DATA,
			None,
			ForkKind::Data,
			|_, _| Ok(()),
			|fork| {
				directory.publish_fork_verified(
					&name,
					fork,
					LIMITS,
					|| false,
					|| {
						let staging = fs::read_dir(&scratch.destination)?.next().unwrap()?.path();
						fs::set_permissions(staging, fs::Permissions::from_mode(mode))
					},
				)
			},
		)
		.unwrap();
		assert!(matches!(
			result,
			Err(ForkWriteError::Host(HostError::StageChanged))
		));
		assert_children(&scratch.destination, &[]);
	}
	scratch.outside_untouched();
}
