use std::{fs::File, io};

#[cfg(target_os = "macos")]
use rustix::{fs, io::Errno};

#[cfg(target_os = "macos")]
const APPLE_QUARANTINE: &str = "com.apple.quarantine";

/// Checks for the native macOS quarantine marker on the held source image.
/// The Linux host profile has no equivalent uniform native marker.
///
/// # Errors
/// Reports a present marker or an unavailable attribute check on macOS.
pub fn check_source_quarantine(file: &File) -> io::Result<()> {
	#[cfg(target_os = "macos")]
	match fs::fgetxattr_query(file, APPLE_QUARANTINE) {
		Err(Errno::NOATTR) => Ok(()),
		Err(error) => Err(error.into()),
		Ok(_) => Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"extraction cannot preserve source image quarantine",
		)),
	}
	#[cfg(target_os = "linux")]
	{
		let _ = file;
		Ok(())
	}
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn refuses_a_quarantine_marker_on_the_admitted_descriptor() {
	const SYNTHETIC_MARKER: &[u8] = b"0081;00000000;dmgez-test;";
	let directory = tempfile::tempdir().unwrap();
	let original = directory.path().join("original.dmg");
	let moved = directory.path().join("moved.dmg");
	let file = File::create(&original).unwrap();
	check_source_quarantine(&file).unwrap();
	fs::fsetxattr(
		&file,
		APPLE_QUARANTINE,
		SYNTHETIC_MARKER,
		fs::XattrFlags::empty(),
	)
	.unwrap();
	std::fs::rename(&original, moved).unwrap();
	let replacement = File::create(original).unwrap();
	check_source_quarantine(&replacement).unwrap();
	let refusal = check_source_quarantine(&file).unwrap_err();
	assert_eq!(refusal.kind(), io::ErrorKind::PermissionDenied);
}
