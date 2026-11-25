//! Native filesystem checks for D-V06 and D-BUILD-05-T02/T03.

#![cfg(all(feature = "staging", any(target_os = "linux", target_os = "macos")))]

use std::{
	fs::{self, File},
	io::{self, Read, Write},
	os::unix::fs::{MetadataExt, symlink},
	path::{Path, PathBuf},
};

use dmgez_core::output::{
	PublishError, StagedReader,
	replace::{EntryIdentity, ReplaceError, TargetIdentity, publish_replace},
};

const DESTINATION: &str = "product.dmg";
const INPUT: &str = "input.bin";
const PREVIOUS: &[u8] = b"previous image";
const STAGED: &[u8] = b"synced, verified image";
const CONCURRENT: &[u8] = b"concurrent actor's output";
const NO_SOURCE_ALIAS: fn(TargetIdentity) -> io::Result<()> = |_| Ok(());
const VERIFY_COMPLETE: fn(StagedReader<'_>, &usize) -> io::Result<()> = |mut staged, count| {
	let mut bytes = Vec::new();
	staged.read_to_end(&mut bytes)?;
	assert_eq!(bytes, STAGED);
	assert_eq!(bytes.len(), *count);
	Ok(())
};

fn fixture() -> io::Result<(tempfile::TempDir, PathBuf)> {
	let directory = tempfile::tempdir()?;
	let path = directory.path().canonicalize()?;
	Ok((directory, path))
}

fn write_image(stage: &mut dyn Write) -> io::Result<usize> {
	stage.write_all(STAGED)?;
	Ok(STAGED.len())
}

fn assert_directory_len(directory: &Path, entries: usize) -> io::Result<()> {
	assert_eq!(fs::read_dir(directory)?.count(), entries);
	Ok(())
}

#[test]
fn replaces_after_readonly_verification_and_preserves_open_old_handles() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	let mut original = File::open(&destination)?;
	let report = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		write_image,
		|stage, report| {
			assert_eq!(fs::read(&destination)?, PREVIOUS);
			VERIFY_COMPLETE(stage, report)
		},
	)
	.unwrap();
	let mut original_bytes = Vec::new();
	original.read_to_end(&mut original_bytes)?;
	assert_eq!(original_bytes, PREVIOUS);
	assert_eq!(report, STAGED.len());
	assert_eq!(fs::read(destination)?, STAGED);
	assert_directory_len(&directory, 1)
}

#[test]
fn failed_verification_cancellation_and_write_preserve_previous_bytes() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	for kind in [io::ErrorKind::InvalidData, io::ErrorKind::Interrupted] {
		let failed = publish_replace(
			&destination,
			NO_SOURCE_ALIAS,
			write_image,
			|stage, report| {
				VERIFY_COMPLETE(stage, report)?;
				Err(kind.into())
			},
		);
		assert!(
			matches!(failed, Err(ReplaceError::Stage(PublishError::Validate(error))) if error.kind() == kind)
		);
		assert_eq!(fs::read(&destination)?, PREVIOUS);
		assert_directory_len(&directory, 1)?;
	}
	let failed = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		|stage| {
			stage.write_all(STAGED)?;
			Err(io::ErrorKind::StorageFull.into())
		},
		|_, _: &()| unreachable!("write failed before verification"),
	);
	assert!(
		matches!(failed, Err(ReplaceError::Stage(PublishError::Write(error))) if error.kind() == io::ErrorKind::StorageFull)
	);
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_directory_len(&directory, 1)
}

#[test]
fn rejects_symlinks_and_guard_rejects_protected_hard_link() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let source = directory.join(INPUT);
	let destination = directory.join(DESTINATION);
	fs::write(&source, PREVIOUS)?;
	let info = fs::metadata(&source)?;
	let identity = EntryIdentity {
		device: info.dev(),
		inode: info.ino(),
	};
	symlink(&source, &destination)?;
	let rejected = publish_replace(&destination, NO_SOURCE_ALIAS, write_image, VERIFY_COMPLETE);
	assert!(matches!(rejected, Err(ReplaceError::DestinationType)));
	assert_eq!(fs::read_link(&destination)?, source);
	fs::remove_file(&destination)?;
	fs::hard_link(&source, &destination)?;
	let forbidden = |target: TargetIdentity| {
		assert_eq!(target.existing, Some(identity));
		Err(io::ErrorKind::InvalidInput.into())
	};
	let rejected = publish_replace(&destination, forbidden, write_image, VERIFY_COMPLETE);
	assert!(
		matches!(rejected, Err(ReplaceError::Authorize(error)) if error.kind() == io::ErrorKind::InvalidInput)
	);
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_eq!(fs::read(source)?, PREVIOUS);
	assert_directory_len(&directory, 2)
}

#[test]
fn creates_absent_destination_and_rejects_entries_created_during_verification() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	let raced = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		write_image,
		|stage, report| {
			VERIFY_COMPLETE(stage, report)?;
			fs::write(&destination, CONCURRENT)
		},
	);
	assert!(matches!(raced, Err(ReplaceError::DestinationChanged)));
	assert_eq!(fs::read(&destination)?, CONCURRENT);
	assert_directory_len(&directory, 1)?;
	fs::remove_file(&destination)?;
	let report =
		publish_replace(&destination, NO_SOURCE_ALIAS, write_image, VERIFY_COMPLETE).unwrap();
	assert_eq!(report, STAGED.len());
	assert_eq!(fs::read(destination)?, STAGED);
	assert_directory_len(&directory, 1)
}

#[test]
fn rejects_mutation_of_a_present_destination_and_serializes_cooperative_publishers()
-> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	let raced = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		write_image,
		|stage, report| {
			VERIFY_COMPLETE(stage, report)?;
			let competing =
				publish_replace(&destination, NO_SOURCE_ALIAS, write_image, VERIFY_COMPLETE);
			assert!(matches!(competing, Err(ReplaceError::Busy)));
			fs::write(&destination, CONCURRENT)
		},
	);
	assert!(matches!(raced, Err(ReplaceError::DestinationChanged)));
	assert_eq!(fs::read(destination)?, CONCURRENT);
	assert_directory_len(&directory, 1)
}

#[test]
fn moved_directory_cannot_redirect_commit_or_stage_cleanup() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let selected = directory.join("selected");
	let moved = directory.join("moved");
	let others = directory.join("foreign");
	fs::create_dir(&selected)?;
	fs::create_dir(&others)?;
	let destination = selected.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	fs::write(others.join(DESTINATION), CONCURRENT)?;
	let raced = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		write_image,
		|stage, report| {
			VERIFY_COMPLETE(stage, report)?;
			fs::rename(&selected, &moved)?;
			symlink(&others, &selected)
		},
	);
	assert!(matches!(raced, Err(ReplaceError::DirectoryChanged)));
	assert_eq!(fs::read(moved.join(DESTINATION))?, PREVIOUS);
	assert_eq!(fs::read(others.join(DESTINATION))?, CONCURRENT);
	assert_directory_len(&moved, 1)?;
	assert_directory_len(&others, 1)
}

#[test]
fn changed_stage_is_rejected_without_cleaning_a_substituted_entry() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	let mut substituted = PathBuf::new();
	let raced = publish_replace(
		&destination,
		NO_SOURCE_ALIAS,
		write_image,
		|stage, report| {
			VERIFY_COMPLETE(stage, report)?;
			for entry in fs::read_dir(&directory)? {
				let path = entry?.path();
				if path != destination {
					substituted = path;
					break;
				}
			}
			fs::remove_file(&substituted)?;
			fs::write(&substituted, CONCURRENT)
		},
	);
	assert!(matches!(raced, Err(ReplaceError::StageChanged)));
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_eq!(fs::read(substituted)?, CONCURRENT);
	assert_directory_len(&directory, 2)
}

#[test]
fn directory_shaped_spellings_cannot_replace_a_regular_output() -> io::Result<()> {
	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	for suffix in ["/", "/.", "/.."] {
		let mut path = destination.as_os_str().to_os_string();
		path.push(suffix);
		let error = publish_replace(
			Path::new(&path),
			NO_SOURCE_ALIAS,
			write_image,
			VERIFY_COMPLETE,
		);
		assert!(matches!(error, Err(ReplaceError::InvalidPath)));
	}
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_directory_len(&directory, 1)
}

#[cfg(any(target_os = "linux", feature = "native-directory-privacy"))]
#[test]
fn checked_replacement_rejects_a_change_during_staging() -> io::Result<()> {
	use dmgez_core::output::replace::publish_replace_existing;

	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	let metadata = fs::metadata(&destination)?;
	let expected = EntryIdentity {
		device: metadata.dev(),
		inode: metadata.ino(),
	};
	let failure = publish_replace_existing(
		&destination,
		expected,
		|original| {
			let mut data = Vec::new();
			original.read_to_end(&mut data)?;
			assert_eq!(data, PREVIOUS);
			Ok(())
		},
		|stage| {
			let report = write_image(stage)?;
			fs::write(&destination, CONCURRENT)?;
			Ok(report)
		},
		VERIFY_COMPLETE,
	);
	assert!(matches!(failure, Err(ReplaceError::DestinationChanged)));
	assert_eq!(fs::read(destination)?, CONCURRENT);
	assert_directory_len(&directory, 1)
}

#[cfg(any(target_os = "linux", feature = "native-directory-privacy"))]
#[test]
fn checked_replacement_refuses_a_fifo_and_extended_metadata() -> io::Result<()> {
	use std::process::Command;

	use dmgez_core::output::replace::publish_replace_existing;
	use rustix::fs::{XattrFlags, fsetxattr};

	let (_fixture, directory) = fixture()?;
	let destination = directory.join(DESTINATION);
	let authorize = |_: &mut dyn Read| panic!("nonqualifying incumbent reached read callback");
	fs::write(&destination, PREVIOUS)?;
	let metadata = fs::metadata(&destination)?;
	let expected = EntryIdentity {
		device: metadata.dev(),
		inode: metadata.ino(),
	};
	fsetxattr(
		File::open(&destination)?,
		"user.dmgez-project-note",
		b"keep",
		XattrFlags::empty(),
	)?;
	let failure = publish_replace_existing(
		&destination,
		expected,
		authorize,
		write_image,
		VERIFY_COMPLETE,
	);
	assert!(matches!(failure, Err(ReplaceError::DestinationMetadata)));
	assert_eq!(fs::read(&destination)?, PREVIOUS);
	fs::remove_file(&destination)?;
	// The portable std API has no FIFO constructor.
	assert!(Command::new("mkfifo").arg(&destination).status()?.success());
	let failure = publish_replace_existing(
		&destination,
		expected,
		authorize,
		write_image,
		VERIFY_COMPLETE,
	);
	assert!(matches!(failure, Err(ReplaceError::DestinationType)));
	assert_directory_len(&directory, 1)
}
