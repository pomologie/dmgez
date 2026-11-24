//! Integration checks for the publication portion of D-V06.

#![cfg(feature = "staging")]

use std::{
	fs,
	io::{self, Read},
};

use dmgez_core::output::{PublishError, publish_new};

const DESTINATION: &str = "product.dmg";
const STAGED: &[u8] = b"complete stage";
const PREVIOUS: &[u8] = b"previous image";

#[test]
fn validates_the_complete_stage_before_it_becomes_visible() -> io::Result<()> {
	let dir = tempfile::tempdir()?;
	let destination = dir.path().join(DESTINATION);
	let report = publish_new(
		&destination,
		|stage| {
			stage.write_all(STAGED)?;
			Ok::<_, io::Error>(STAGED.len())
		},
		|mut reader, bytes| {
			assert!(!destination.exists());
			let mut observed = Vec::new();
			reader.read_to_end(&mut observed)?;
			assert_eq!(&observed, STAGED);
			assert_eq!(observed.len(), *bytes);
			Ok(())
		},
	)
	.unwrap();
	assert_eq!(report, STAGED.len());
	assert_eq!(fs::read(destination)?, STAGED);
	assert_eq!(fs::read_dir(dir.path())?.count(), 1);
	Ok(())
}

#[test]
fn write_and_validation_failures_leave_a_previous_output_intact() -> io::Result<()> {
	let dir = tempfile::tempdir()?;
	let destination = dir.path().join(DESTINATION);
	fs::write(&destination, PREVIOUS)?;
	let error = publish_new(
		&destination,
		|stage| {
			stage.write_all(STAGED)?;
			Err::<(), io::Error>(io::ErrorKind::StorageFull.into())
		},
		|_, _| panic!("must skip validation after write failure"),
	)
	.unwrap_err();
	assert!(
		matches!(error, PublishError::Write(error) if error.kind() == io::ErrorKind::StorageFull)
	);
	assert_eq!(fs::read(&destination)?, PREVIOUS);

	let error = publish_new(
		&destination,
		|stage| stage.write_all(STAGED),
		|_, _| Err(io::ErrorKind::InvalidData.into()),
	)
	.unwrap_err();
	assert!(
		matches!(error, PublishError::Validate(error) if error.kind() == io::ErrorKind::InvalidData)
	);
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_eq!(fs::read_dir(dir.path())?.count(), 1);
	Ok(())
}

#[test]
fn commit_does_not_replace_an_output_created_during_validation() -> io::Result<()> {
	let dir = tempfile::tempdir()?;
	let destination = dir.path().join(DESTINATION);
	let error = publish_new(
		&destination,
		|stage| stage.write_all(STAGED),
		|_, _| fs::write(&destination, PREVIOUS),
	)
	.unwrap_err();
	assert!(
		matches!(error, PublishError::Commit(error) if error.kind() == io::ErrorKind::AlreadyExists)
	);
	assert_eq!(fs::read(destination)?, PREVIOUS);
	assert_eq!(fs::read_dir(dir.path())?.count(), 1);
	Ok(())
}
