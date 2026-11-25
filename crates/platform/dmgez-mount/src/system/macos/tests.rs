use std::{
	cell::{Cell, RefCell},
	fs,
	io::{self, Read},
	os::unix::{fs::symlink, process::ExitStatusExt},
	path::Path,
	process::ExitStatus,
	time::{Duration, UNIX_EPOCH},
};

use dmgez_core::process;

use crate::system::{
	Attachment, DetachOutcome, Error, Observation, check_attachment_source, open_attachment_source,
	record::{DeviceStamp, MountedIdentity},
};

use super::{HeldImage, detach_with};

const IMAGE: &str = "image.dmg";
const MOCK_DEVICE: &str = "/dev/disk42";
const IMAGE_BYTES: &[u8] = b"original image";

fn recorded_attachment(image: &Path) -> Attachment {
	let stamp = HeldImage::open(image).unwrap().stamp;
	Attachment {
		image: image.to_owned().try_into().unwrap(),
		mountpoint: image.with_file_name("mount").try_into().unwrap(),
		device: MOCK_DEVICE.to_owned().try_into().unwrap(),
		source: stamp,
		underlying: stamp.id,
		mount: MountedIdentity {
			root: stamp.id,
			fsid: 0,
		},
		node: DeviceStamp {
			id: stamp.id,
			rdev: 0,
			changed: stamp.changed,
		},
	}
}

#[test]
fn attachment_source_opens_the_recorded_image_and_detects_changes_through_its_name() {
	let temp = tempfile::tempdir().unwrap();
	let image = temp.path().canonicalize().unwrap().join(IMAGE);
	fs::write(&image, IMAGE_BYTES).unwrap();
	let attachment = recorded_attachment(&image);
	let mut file = open_attachment_source(&attachment).unwrap();
	let mut bytes = Vec::new();
	file.read_to_end(&mut bytes).unwrap();
	assert_eq!(bytes, IMAGE_BYTES);
	check_attachment_source(&attachment, &file).unwrap();

	let replacement = [b'x'; IMAGE_BYTES.len()];
	fs::write(&image, replacement).unwrap();
	fs::OpenOptions::new()
		.write(true)
		.open(&image)
		.unwrap()
		.set_modified(UNIX_EPOCH + Duration::from_secs(1))
		.unwrap();
	assert!(matches!(
		check_attachment_source(&attachment, &file),
		Err(Error::IdentityMismatch)
	));
	assert!(matches!(
		open_attachment_source(&attachment),
		Err(Error::IdentityMismatch)
	));
}

#[test]
fn attachment_source_refuses_a_replacement_with_identical_bytes() {
	let temp = tempfile::tempdir().unwrap();
	let image = temp.path().canonicalize().unwrap().join(IMAGE);
	fs::write(&image, IMAGE_BYTES).unwrap();
	let attachment = recorded_attachment(&image);
	let file = open_attachment_source(&attachment).unwrap();

	fs::rename(&image, image.with_file_name("renamed.dmg")).unwrap();
	fs::write(&image, IMAGE_BYTES).unwrap();
	assert!(matches!(
		check_attachment_source(&attachment, &file),
		Err(Error::IdentityMismatch)
	));
	assert!(matches!(
		open_attachment_source(&attachment),
		Err(Error::IdentityMismatch)
	));
}

#[test]
fn attachment_source_refuses_symbolic_aliases_even_when_the_target_stamp_matches() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let image = root.join(IMAGE);
	let alias = root.join("alias.dmg");
	fs::write(&image, IMAGE_BYTES).unwrap();
	symlink(&image, &alias).unwrap();
	let mut attachment = recorded_attachment(&image);
	let file = open_attachment_source(&attachment).unwrap();
	attachment.image = alias.try_into().unwrap();
	assert!(matches!(
		check_attachment_source(&attachment, &file),
		Err(Error::Path { .. })
	));
	assert!(matches!(
		open_attachment_source(&attachment),
		Err(Error::Path { .. })
	));
}

#[test]
fn detach_revalidates_the_mount_then_checks_the_precondition_before_requesting_detach() {
	let event_log = RefCell::new(Vec::new());
	let mounted = Cell::new(true);
	let result = detach_with(
		|| {
			event_log.borrow_mut().push("observe");
			Ok(if mounted.get() {
				Observation::Mounted
			} else {
				Observation::Detached
			})
		},
		|| {
			event_log.borrow_mut().push("check");
			Ok(())
		},
		|| {
			event_log.borrow_mut().push("request");
			mounted.set(false);
			Ok(process::Output {
				status: ExitStatus::from_raw(0),
				stdout: Vec::new(),
				stderr: Vec::new(),
			})
		},
	)
	.unwrap();
	assert_eq!(result, DetachOutcome::Detached);
	assert_eq!(
		event_log.into_inner(),
		["observe", "check", "request", "observe"]
	);
}

#[test]
fn refusing_the_detach_precondition_preserves_the_io_error_without_a_request() {
	let observed = Cell::new(false);
	let result = detach_with(
		|| {
			assert!(!observed.replace(true));
			Ok(Observation::Mounted)
		},
		|| Err(io::ErrorKind::PermissionDenied.into()),
		|| panic!("precondition refusal must skip the native request"),
	);
	assert!(
		matches!(result, Err(Error::DetachPrecondition { source }) if source.kind() == io::ErrorKind::PermissionDenied)
	);
}

#[test]
fn an_already_detached_or_mismatched_attachment_skips_the_precondition_and_request() {
	for observation in [Observation::Detached, Observation::IdentityMismatch] {
		let result = detach_with(
			|| Ok(observation),
			|| panic!("a detached or mismatched attachment must skip the precondition"),
			|| panic!("a detached or mismatched attachment must skip the native request"),
		);
		if observation == Observation::Detached {
			assert_eq!(result.unwrap(), DetachOutcome::AlreadyDetached);
		} else {
			assert!(matches!(result, Err(Error::IdentityMismatch)));
		}
	}
}
