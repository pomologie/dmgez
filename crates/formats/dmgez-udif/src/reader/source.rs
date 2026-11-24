use std::{fs::File, io};

use dmgez_core::source::{CapturedFile, SourceError};

use crate::{Error, validation::checked_range};

/// A finite, immutable, byte-addressed image source.
///
/// Implementors must reject reads outside their length, report short reads as
/// failures, and reject changes to the admitted source. Empty reads at EOF are legal.
pub trait ReadAt {
	/// Size of the admitted source in bytes.
	fn len(&self) -> u64;

	/// Whether the source has zero bytes.
	fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Check the admitted identity before serving cached data. Immutable stores
	/// may use the default.
	///
	/// # Errors
	/// Returns an error if the admitted source has changed or became unavailable.
	fn check_unchanged(&self) -> Result<(), Error> {
		Ok(())
	}

	/// Read exactly the requested range.
	///
	/// # Errors
	/// Returns an error for an out-of-bounds, failed or inconsistent read.
	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error>;
}

impl ReadAt for &[u8] {
	fn len(&self) -> u64 {
		<[u8]>::len(self) as u64
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		let range = admitted_range(offset, output.len() as u64, self.len())?;
		output.copy_from_slice(&self[range.start as usize..range.end as usize]);
		Ok(())
	}
}

/// An admitted regular file opened by handle, with serialized seek/read.
///
/// The caller must ensure the file is immutable. Length and available timestamps
/// are checked before and after reads as a change detector. These checks cannot
/// establish an immutable snapshot against a process able to alter the file and
/// restore its timestamps.
#[derive(Debug)]
pub struct FileSource(CapturedFile);

impl FileSource {
	/// Admit a regular file. The caller retains control over how it was opened.
	///
	/// # Errors
	/// Returns an error for non-regular inputs or unavailable file metadata.
	pub fn new(file: File) -> Result<Self, Error> {
		CapturedFile::new(file).map(Self).map_err(source_error)
	}
}

impl ReadAt for FileSource {
	fn len(&self) -> u64 {
		self.0.len()
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.0.validate().map_err(source_error)
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		self.0.read_exact_at(offset, output).map_err(source_error)
	}
}

fn source_error(error: SourceError) -> Error {
	match error {
		SourceError::NotRegularFile => Error::InvalidFormat("image source must be a regular file"),
		SourceError::Changed => Error::SourceChanged,
		SourceError::OutOfRange {
			offset,
			len,
			file_len,
		} => Error::OutOfRange {
			offset,
			len,
			disk_len: file_len,
		},
		SourceError::Io(error) => error.into(),
		error => io::Error::other(error).into(),
	}
}

pub(super) fn admitted_range(
	offset: u64,
	len: u64,
	disk_len: u64,
) -> Result<std::ops::Range<u64>, Error> {
	checked_range(offset, len, disk_len).ok_or(Error::OutOfRange {
		offset,
		len,
		disk_len,
	})
}
