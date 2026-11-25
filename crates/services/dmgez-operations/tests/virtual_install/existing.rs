use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

use dmgez_operations::{
	extract::host::StableDestination,
	install::{
		CopyLimits, CopyPlan, ExistingBundleError, InstallFailure, execute_copy, prepare_copy,
		verify_existing_bundle,
	},
};

use super::{
	planning::{LINK_TARGET, RESOURCE_BYTES, selection},
	*,
};

const TEST_ATTRIBUTE: &[u8] = b"verified native attribute";
const LINK_NAME: &str = "Library";
const EXTRA_NAME: &str = "extra";
const OLD_CONTENTS: &str = "OldContents";
const DATA_BLOCK_BYTES: usize = 64 << 10;

fn plan<'fixture>(
	reads: &'fixture Cell<usize>,
	program: &'fixture [u8],
) -> (VolumeView<Fixture<'fixture>>, CopyPlan<Node>) {
	let mut view = Fixture {
		link: Some(LINK_TARGET),
		attribute: Some(TEST_ATTRIBUTE),
		resource: Some(RESOURCE_BYTES),
		program: Some(program),
		read_size_limit: DATA_BLOCK_BYTES,
		..Fixture::new(reads)
	}
	.view();
	let application = selection(&mut view);
	let plan = prepare_copy(&mut view, application, CopyLimits::default(), || false).unwrap();
	(view, plan)
}

fn publish<'fixture>(
	parent: &Path,
	reads: &'fixture Cell<usize>,
	program: &'fixture [u8],
) -> (
	StableDestination,
	VolumeView<Fixture<'fixture>>,
	CopyPlan<Node>,
) {
	let (mut view, plan) = plan(reads, program);
	let parent = StableDestination::open(parent).unwrap();
	execute_copy(&mut view, &plan, &parent.directory(), || Ok(()), || false).unwrap();
	reads.set(0);
	(parent, view, plan)
}

#[test]
fn verifies_an_existing_exact_bundle_with_bounded_source_reads() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let contents = vec![0x5a; 2 * DATA_BLOCK_BYTES + 1];
	let reads = Cell::new(0);
	let (destination, mut view, plan) = publish(&parent, &reads, &contents);
	let verified = verify_existing_bundle(
		&mut view,
		&plan,
		&destination.directory(),
		|| Ok(()),
		|| false,
	)
	.unwrap();
	assert_eq!(verified, plan.summary());
	// Info.plist, three data chunks, one resource fork, and one named attribute.
	assert_eq!(reads.get(), 6);
	assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
}

#[test]
fn distinguishes_an_absent_root_from_an_extra_or_missing_child() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let reads = Cell::new(0);
	let (mut view, plan) = plan(&reads, PROGRAM_BYTES);
	let held_root = StableDestination::open(&parent).unwrap();
	let destination = held_root.directory();
	let mut verify = || verify_existing_bundle(&mut view, &plan, &destination, || Ok(()), || false);
	assert!(matches!(verify(), Err(ExistingBundleError::Missing)));
	execute_copy(&mut view, &plan, &destination, || Ok(()), || false).unwrap();
	let program = parent.join(APP_NAME).join(CONTENTS_NAME).join(INFO_NAME);
	let extra = parent.join(APP_NAME).join(EXTRA_NAME);
	fs::write(&extra, b"unplanned").unwrap();
	let mut verify = || verify_existing_bundle(&mut view, &plan, &destination, || Ok(()), || false);
	assert!(matches!(verify(), Err(ExistingBundleError::Mismatch)));
	fs::remove_file(&extra).unwrap();
	fs::remove_file(&program).unwrap();
	assert!(matches!(verify(), Err(ExistingBundleError::Mismatch)));
}

#[test]
fn rejects_links_and_special_entries_by_kind_before_reading_file_payload() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let reads = Cell::new(0);
	let (destination, mut view, plan) = publish(&parent, &reads, PROGRAM_BYTES);
	let info = parent.join(APP_NAME).join(CONTENTS_NAME).join(INFO_NAME);
	let link = parent.join(APP_NAME).join(CONTENTS_NAME).join(LINK_NAME);
	fs::remove_file(&link).unwrap();
	symlink("../different", &link).unwrap();
	let mut verify = || {
		verify_existing_bundle(
			&mut view,
			&plan,
			&destination.directory(),
			|| Ok(()),
			|| false,
		)
	};
	assert!(matches!(verify(), Err(ExistingBundleError::Mismatch)));
	fs::remove_file(&link).unwrap();
	symlink(str::from_utf8(LINK_TARGET).unwrap(), &link).unwrap();
	fs::remove_file(&info).unwrap();
	symlink(parent.join(APP_NAME), &info).unwrap();
	reads.set(0);
	assert!(matches!(verify(), Err(ExistingBundleError::Mismatch)));
	assert_eq!(reads.get(), 0);
	fs::remove_file(&info).unwrap();
	assert!(
		Command::new("/usr/bin/mkfifo")
			.args(["-m", "644"])
			.arg(&info)
			.status()
			.unwrap()
			.success()
	);
	assert!(matches!(verify(), Err(ExistingBundleError::Mismatch)));
	assert_eq!(reads.get(), 0);
}

#[test]
fn rejects_late_same_size_rewrite_of_an_earlier_verified_file() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let reads = Cell::new(0);
	let (destination, mut view, plan) = publish(&parent, &reads, PROGRAM_BYTES);
	let info = parent.join(APP_NAME).join(CONTENTS_NAME).join(INFO_NAME);
	let initial_inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&info).unwrap());
	let mut changed = false;
	let result = verify_existing_bundle(
		&mut view,
		&plan,
		&destination.directory(),
		|| {
			if reads.get() >= 4 && !changed {
				fs::write(&info, vec![b'x'; INFO.len()])?;
				changed = true;
			}
			Ok(())
		},
		|| false,
	);
	assert!(changed);
	assert_eq!(fs::metadata(&info).unwrap().len(), INFO.len() as u64);
	assert_eq!(
		std::os::unix::fs::MetadataExt::ino(&fs::metadata(&info).unwrap()),
		initial_inode,
	);
	assert!(
		matches!(result, Err(ExistingBundleError::Changed)),
		"{result:?}"
	);
}

#[test]
fn rejects_a_late_directory_replacement_and_reports_cancellation() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let reads = Cell::new(0);
	let (destination, mut view, plan) = publish(&parent, &reads, PROGRAM_BYTES);
	let contents = parent.join(APP_NAME).join(CONTENTS_NAME);
	let moved = parent.join(OLD_CONTENTS);
	let result = verify_existing_bundle(
		&mut view,
		&plan,
		&destination.directory(),
		|| {
			if reads.get() >= 4 && !moved.exists() {
				fs::rename(&contents, &moved)?;
				fs::create_dir(&contents)?;
			}
			Ok(())
		},
		|| false,
	);
	assert!(moved.join(INFO_NAME).is_file());
	assert!(
		matches!(result, Err(ExistingBundleError::Changed)),
		"{result:?}"
	);
	assert!(matches!(
		verify_existing_bundle(
			&mut view,
			&plan,
			&destination.directory(),
			|| Ok(()),
			|| true
		),
		Err(ExistingBundleError::Cancelled),
	));
}

#[test]
fn requires_held_source_revalidation_after_comparing_payloads() {
	let root = tempfile::tempdir().unwrap();
	let parent = root.path().canonicalize().unwrap();
	let reads = Cell::new(0);
	let (destination, mut view, plan) = publish(&parent, &reads, PROGRAM_BYTES);
	let result = verify_existing_bundle(
		&mut view,
		&plan,
		&destination.directory(),
		|| {
			if reads.get() >= 4 {
				return Err(io::ErrorKind::InvalidData.into());
			}
			Ok(())
		},
		|| false,
	);
	assert!(matches!(
		result,
		Err(ExistingBundleError::Check(InstallFailure::SourceIdentity(
			_
		))),
	));
}
