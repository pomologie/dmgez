use std::{fs, os::unix::fs::symlink};

use rustix::fs::{Mode, fchmod, fstat};

use super::fixture::{DESTINATION, DETACHED, SENTINEL, Scratch, assert_children, host_name};
use crate::extract::host::HostError;

const BUNDLE: &str = "Example.app";
const PUBLISHED_MODE: Mode = Mode::RUSR
	.union(Mode::WUSR)
	.union(Mode::XUSR)
	.union(Mode::RGRP)
	.union(Mode::XGRP)
	.union(Mode::ROTH)
	.union(Mode::XOTH);

#[test]
fn publishes_the_held_bundle_and_removes_the_private_wrapper() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let mut stage = directory.stage_directory(&host_name(BUNDLE)).unwrap();
	let wrapper = scratch.destination.join(stage.name().as_str());
	let private = wrapper.join(BUNDLE);
	let inode = fstat(stage.directory().descriptor()).unwrap().st_ino;
	fchmod(stage.directory().descriptor(), PUBLISHED_MODE).unwrap();
	fs::write(private.join(DESTINATION), SENTINEL).unwrap();
	assert_children(&scratch.destination, &[stage.name().as_str()]);
	assert!(!stage.is_published());

	stage.publish().unwrap();
	assert!(stage.is_published());
	stage.cleanup().unwrap();
	assert!(stage.is_published());
	assert_children(&scratch.destination, &[BUNDLE]);
	let output = scratch.destination.join(BUNDLE);
	assert_eq!(fs::read(output.join(DESTINATION)).unwrap(), SENTINEL);
	let file = fs::File::open(&output).unwrap();
	let stat = fstat(&file).unwrap();
	assert_eq!(stat.st_ino, inode);
	assert_eq!(Mode::from_raw_mode(stat.st_mode), PUBLISHED_MODE);
}

#[test]
fn late_conflict_preserves_the_incumbent_and_nonempty_stage() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let mut stage = directory.stage_directory(&host_name(BUNDLE)).unwrap();
	let wrapper = scratch.destination.join(stage.name().as_str());
	let private_file = wrapper.join(BUNDLE).join(DESTINATION);
	let incumbent = scratch.destination.join(BUNDLE);
	fs::write(&private_file, SENTINEL).unwrap();
	symlink(scratch.outside.join(BUNDLE), &incumbent).unwrap();

	assert!(matches!(stage.publish(), Err(HostError::Conflict)));
	assert!(!stage.is_published());
	assert!(matches!(
		stage.cleanup(),
		Err(HostError::RemoveDirectoryStage(_))
	));
	assert!(incumbent.is_symlink());
	assert_eq!(fs::read(&private_file).unwrap(), SENTINEL);
	fs::remove_file(private_file).unwrap();
	stage.cleanup().unwrap();
	assert_children(&scratch.destination, &[BUNDLE]);
	scratch.outside_untouched();
}

#[test]
fn dropping_an_abandoned_empty_stage_leaves_no_destination_entry() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let stage = directory.stage_directory(&host_name(BUNDLE)).unwrap();
	assert!(!stage.is_published());
	drop(stage);
	assert_children(&scratch.destination, &[]);
}

#[test]
fn replaced_wrapper_blocks_publication_and_cleanup() {
	let scratch = Scratch::new();
	let root = scratch.open();
	let directory = root.directory();
	let mut stage = directory.stage_directory(&host_name(BUNDLE)).unwrap();
	let wrapper = scratch.destination.join(stage.name().as_str());
	let detached = scratch.destination.join(DETACHED);
	fs::rename(&wrapper, &detached).unwrap();
	fs::create_dir(&wrapper).unwrap();
	fs::create_dir(wrapper.join(BUNDLE)).unwrap();

	assert!(matches!(stage.publish(), Err(HostError::Detached)));
	assert!(matches!(stage.cleanup(), Err(HostError::Detached)));
	assert!(!stage.is_published());
	drop(stage);
	assert_children(&wrapper, &[BUNDLE]);
	assert_children(&detached, &[BUNDLE]);
	assert!(!scratch.destination.join(BUNDLE).exists());
}
