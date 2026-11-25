//! Explicit project saves guarded by the opened file's identity and exact text.

use std::{fs::Metadata, path::Path};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
	io::{self, Read},
	os::unix::fs::MetadataExt as _,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use dmgez_config::MAX_MANIFEST_BYTES;
use dmgez_config::project_edit::ProjectEditError;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use dmgez_core::output::replace::{EntryIdentity, ReplaceError, publish_replace_existing};
use thiserror::Error;

#[cfg(any(target_os = "linux", target_os = "macos"))]
const COMPARE_BYTES: usize = 4096;

/// Filesystem identity associated with one saved snapshot of a project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileRevision {
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	identity: EntryIdentity,
}

impl FileRevision {
	/// Captures the identity from metadata on the same handle used to read the text.
	#[must_use]
	pub fn from_metadata(metadata: &Metadata) -> Self {
		#[cfg(not(any(target_os = "linux", target_os = "macos")))]
		let _ = metadata;
		Self {
			#[cfg(any(target_os = "linux", target_os = "macos"))]
			identity: EntryIdentity {
				device: metadata.dev(),
				inode: metadata.ino(),
			},
		}
	}
}

/// Failure to validate or publish the requested project revision.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("saving over the open project is unavailable on this platform")]
	Unsupported,
	#[error("the project changed on disk; reopen it or save a copy")]
	Conflict,
	#[error("the saved project exceeds the manifest size limit")]
	OriginalTooLarge,
	#[error(transparent)]
	Project(#[from] ProjectEditError),
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[error("could not save the project: {0}")]
	Publication(#[source] ReplaceError<ContentError>),
}

/// Failure while comparing or writing one of the file revisions.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContentError {
	#[error("the file content differs from the expected revision")]
	Mismatch,
	#[error(transparent)]
	Io(#[from] io::Error),
}

/// Whether existing-file save is available on the current platform.
#[must_use]
pub const fn replace_supported() -> bool {
	cfg!(any(target_os = "linux", target_os = "macos"))
}

/// Validates and saves the exact edited TOML text to the open project's path.
///
/// The caller supplies the file identity and exact text last observed together.
/// The returned identity must accompany the edited text in subsequent saves.
/// On macOS and Linux this uses a trusted local directory and the cooperative
/// writer locking contract of dmgez-core. Ordinary permissions and ownership
/// are retained. Files with hard links or unsupported extended metadata are refused.
///
/// # Errors
/// Rejects unsupported platforms, invalid TOML, stale identity or text, and
/// filesystems that cannot safely replace the incumbent.
pub fn save(
	path: &Path,
	base: FileRevision,
	expected: &str,
	edited: &str,
) -> Result<FileRevision, Error> {
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	{
		use dmgez_config::project_edit::ProjectDocument;

		if expected.len() > MAX_MANIFEST_BYTES {
			return Err(Error::OriginalTooLarge);
		}
		ProjectDocument::open(edited)?;
		let (_, identity) = publish_replace_existing(
			path,
			base.identity,
			|original| compare(original, expected.as_bytes()),
			|staged| {
				staged
					.write_all(edited.as_bytes())
					.map_err(ContentError::from)
			},
			|mut staged, ()| compare(&mut staged, edited.as_bytes()),
		)
		.map_err(|error| match error {
			ReplaceError::DestinationChanged
			| ReplaceError::DestinationType
			| ReplaceError::Authorize(ContentError::Mismatch) => Error::Conflict,
			other => Error::Publication(other),
		})?;
		Ok(FileRevision { identity })
	}
	#[cfg(not(any(target_os = "linux", target_os = "macos")))]
	{
		let _ = (path, base, expected, edited);
		Err(Error::Unsupported)
	}
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn compare(input: &mut dyn Read, expected: &[u8]) -> Result<(), ContentError> {
	let mut buffer = [0; COMPARE_BYTES];
	for chunk in expected.chunks(buffer.len()) {
		let data = &mut buffer[..chunk.len()];
		if let Err(error) = input.read_exact(data) {
			return Err(if error.kind() == io::ErrorKind::UnexpectedEof {
				ContentError::Mismatch
			} else {
				error.into()
			});
		}
		if data != chunk {
			return Err(ContentError::Mismatch);
		}
	}
	match input.read_exact(&mut buffer[..1]) {
		Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
		Err(error) => Err(error.into()),
		Ok(()) => Err(ContentError::Mismatch),
	}
}
