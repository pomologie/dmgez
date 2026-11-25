//! Read-only attachment through the native macOS disk-image service.

use std::{
	fs::File,
	io,
	path::{Path, PathBuf},
};

#[cfg(target_os = "macos")]
mod macos;
mod record;

pub use record::Attachment;
#[cfg(target_os = "macos")]
pub(crate) use record::valid_native_path;

/// An image and an explicit empty mountpoint, both named by canonical paths.
#[derive(Clone, Debug)]
pub struct Request {
	image: PathBuf,
	mountpoint: PathBuf,
}

impl Request {
	/// Selects the paths that will be validated before attachment.
	#[must_use]
	pub fn new(image: impl Into<PathBuf>, mountpoint: impl Into<PathBuf>) -> Self {
		Self {
			image: image.into(),
			mountpoint: mountpoint.into(),
		}
	}

	/// The selected image path.
	#[must_use]
	pub fn image(&self) -> &Path {
		&self.image
	}

	/// The selected mountpoint.
	#[must_use]
	pub fn mountpoint(&self) -> &Path {
		&self.mountpoint
	}
}

/// The current relationship between recorded attachment evidence and the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Observation {
	/// The original read-only attachment is still present.
	Mounted,
	/// The original mountpoint is visible and the image and device are absent.
	Detached,
	/// The original relationship cannot be established from current host state.
	IdentityMismatch,
}

/// The independently observed result of asking to detach an attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetachOutcome {
	/// The original attachment was absent after a native detach attempt.
	Detached,
	/// The attachment was already absent. No detach was attempted.
	AlreadyDetached,
}

/// Refusal or failure to establish the requested mount state.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	#[error("native read-only disk image mounting is available only on macOS")]
	Unavailable,
	#[error("expected an exact canonical path at {path:?}")]
	Path { path: PathBuf },
	#[error("expected a regular disk image at {path:?}")]
	NotRegular { path: PathBuf },
	#[error("mountpoint must be an empty directory with no existing mount at {path:?}")]
	MountpointOccupied { path: PathBuf },
	#[error("could not confirm the selected image or mountpoint identity")]
	IdentityMismatch,
	#[error("the selected image or mountpoint already appears in the disk-image inventory")]
	AlreadyAttached,
	#[error(
		"native attachment requires a single partitionless HFS Plus volume in a supported UDIF image"
	)]
	UnsupportedImage,
	#[error("could not access {path:?}")]
	Io {
		path: PathBuf,
		#[source]
		source: std::io::Error,
	},
	#[error("native disk-image inventory failed with status {status}")]
	InventoryRefused { status: std::process::ExitStatus },
	#[error("native disk-image service returned invalid or ambiguous attachment evidence")]
	InvalidEvidence,
	#[error("native disk-image service declined attachment with status {status}")]
	AttachRefused { status: std::process::ExitStatus },
	#[error("the attach effect is uncertain; the image may still be attached")]
	AttachUncertain {
		#[source]
		source: Box<Self>,
	},
	#[error("the original disk image is busy and is still mounted")]
	DetachBusy,
	#[error("the final detach precondition was refused")]
	DetachPrecondition {
		#[source]
		source: io::Error,
	},
	#[error(
		"native disk-image service declined to detach a still-mounted image with status {status}"
	)]
	DetachRefused { status: std::process::ExitStatus },
	#[error("could not confirm the detach effect")]
	DetachUncertain {
		#[source]
		source: Box<Self>,
	},
	#[cfg(target_os = "macos")]
	#[error("native disk-image service execution failed")]
	Tool(#[from] dmgez_core::process::Error),
}

impl Error {
	/// Whether the attach call issued a request whose effect could not be confirmed.
	#[must_use]
	pub const fn attachment_may_exist(&self) -> bool {
		matches!(self, Self::AttachUncertain { .. })
	}
}

/// Attaches a supported image read-only and returns independently checked evidence.
///
/// Keep the selected paths stable. The native service resolves them after validation.
///
/// # Errors
/// Returns a refusal for an unsupported image, an unsafe path, or an existing
/// attachment. An issued request whose effect is unknown returns
/// [`Error::AttachUncertain`]. Other platforms return [`Error::Unavailable`].
pub fn attach(request: &Request) -> Result<Attachment, Error> {
	#[cfg(target_os = "macos")]
	return macos::attach(request);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = request;
		Err(Error::Unavailable)
	}
}

/// Checks the recorded image, mountpoint, device, and kernel mount identity.
///
/// # Errors
/// Returns an error when an inventory or filesystem observation fails.
pub fn observe(attachment: &Attachment) -> Result<Observation, Error> {
	#[cfg(target_os = "macos")]
	return macos::observe(attachment);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = attachment;
		Err(Error::Unavailable)
	}
}

/// Opens the recorded image read-only after checking its file stamp and canonical name.
///
/// Use [`observe`] to check the current OS attachment independently. Keep the returned
/// file open while using it as an image source, then call [`check_attachment_source`]
/// before relying on evidence read from the file.
///
/// # Errors
/// Refuses a changed file, unsafe path, or failed filesystem observation. Other
/// platforms return [`Error::Unavailable`].
pub fn open_attachment_source(attachment: &Attachment) -> Result<File, Error> {
	#[cfg(target_os = "macos")]
	return macos::open_attachment_source(attachment);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = attachment;
		Err(Error::Unavailable)
	}
}

/// Checks that the held image and its exact canonical name still match the recorded file stamp.
///
/// # Errors
/// Refuses a changed file, unsafe path, or failed filesystem observation. Other
/// platforms return [`Error::Unavailable`].
pub fn check_attachment_source(attachment: &Attachment, file: &File) -> Result<(), Error> {
	#[cfg(target_os = "macos")]
	return macos::check_attachment_source(attachment, file);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (attachment, file);
		Err(Error::Unavailable)
	}
}

/// Asks the native service to detach the observed image without force.
///
/// Revalidation and the native request are separate operations. External
/// software that changes disk-image attachments must coordinate with the caller.
///
/// # Errors
/// Refuses to detach if the current state cannot be tied to the saved evidence.
/// If the native request fails, the result reports whether detachment could be
/// confirmed. Other platforms return [`Error::Unavailable`].
pub fn detach(attachment: &Attachment) -> Result<(), Error> {
	detach_after(attachment, || Ok(())).map(|_| ())
}

/// Asks the native service to detach a still-mounted image after a final caller check.
///
/// The callback runs only after confirming the original attachment is still mounted
/// and immediately before attempting the non-forced native detach. If observation
/// confirms the attachment is already absent, the callback is skipped and this
/// returns [`DetachOutcome::AlreadyDetached`]. Keep the callback brief and coordinate
/// changes to disk-image attachments with external software.
///
/// # Errors
/// Returns [`Error::DetachPrecondition`] without attempting a detach if the callback
/// fails. Observation and detach errors have the same meaning as for [`detach`].
pub fn detach_after(
	attachment: &Attachment,
	before_request: impl FnOnce() -> io::Result<()>,
) -> Result<DetachOutcome, Error> {
	#[cfg(target_os = "macos")]
	return macos::detach_after(attachment, before_request);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (attachment, before_request);
		Err(Error::Unavailable)
	}
}
