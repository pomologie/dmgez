//! Bounded access to the logical bytes and container checksums of a DMG.

use std::{error::Error as StdError, fmt, path::Path};

use dmgez_udif::reader::{CheckStatus, FileSource, Image, Limits};

/// A read-only disk image backed by an open file handle.
#[derive(Debug)]
pub struct Dmg(Image<FileSource>);

/// An image admission, read, or verification failure.
#[derive(Debug)]
pub struct Error(dmgez_udif::Error);

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}

impl StdError for Error {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		Some(&self.0)
	}
}

/// Results of checking the checksums declared by the container.
#[derive(Debug)]
#[non_exhaustive]
pub struct Verification {
	/// Whether a data-fork checksum was declared and matched.
	pub data_fork: bool,
	/// Whether a master checksum was declared and matched.
	pub master: bool,
	/// Whether each table declared a checksum that matched, in resource order.
	pub tables: Vec<bool>,
}

impl Dmg {
	/// Open a signed or unsigned image using the reader's default resource limits.
	/// Signed images are checked for structural admission at this step.
	///
	/// # Errors
	///
	/// Returns an error for an inaccessible, malformed, unsupported, or
	/// over-budget image.
	pub fn open(path: &Path) -> Result<Self, Error> {
		let source = dmgez_runtime::image::open_file(path).map_err(Error)?;
		Image::open_release(source, Limits::default())
			.map(Self)
			.map_err(Error)
	}

	/// Length of the decoded logical disk in bytes.
	#[must_use]
	pub fn logical_bytes(&self) -> u64 {
		self.0.descriptor().logical_bytes
	}

	/// Length of the container file at admission in bytes.
	#[must_use]
	pub fn physical_bytes(&self) -> u64 {
		self.0.descriptor().physical_bytes
	}

	/// Read exactly the requested bytes at a logical disk offset.
	///
	/// # Errors
	///
	/// Returns an error for a changed source, an out-of-range request, bad
	/// compressed data, or unreadable backing storage. Discard the buffer on error.
	pub fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		self.0.read_exact_at(offset, output).map_err(Error)
	}

	/// Check all declared container checksum domains. Filesystem consistency
	/// and signature authenticity are separate checks.
	///
	/// # Errors
	///
	/// Returns an error if verification encounters an unreadable run, an
	/// unsupported checksum, or a checksum mismatch.
	pub fn verify(&mut self) -> Result<Verification, Error> {
		let checks = self.0.verify().map_err(Error)?;
		Ok(Verification {
			data_fork: checks.data_fork == CheckStatus::Passed,
			master: checks.master == CheckStatus::Passed,
			tables: checks
				.tables
				.into_iter()
				.map(|status| status == CheckStatus::Passed)
				.collect(),
		})
	}
}
