//! Bounded reads and mutation checks on already-opened host files (C2 and C3).

#![forbid(unsafe_code)]

use std::{
	fs::{File, Metadata},
	io::{self, Read, Seek, SeekFrom},
	sync::{Mutex, MutexGuard},
	time::SystemTime,
};

use thiserror::Error;

/// A failure admitting or reading a captured regular file.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceError {
	#[error("source must be a regular file")]
	NotRegularFile,
	#[error("source changed after admission")]
	Changed,
	#[error("source byte range {offset} + {len} exceeds the admitted length {file_len}")]
	OutOfRange {
		offset: u64,
		len: u64,
		file_len: u64,
	},
	#[error(transparent)]
	Io(#[from] io::Error),
}

/// A finite regular-file source tied to an open handle.
///
/// Length and available timestamps are checked before and after reads. A
/// caller requiring protection from writers that can restore timestamps
/// needs an immutable backing store or an independently verified snapshot.
#[derive(Debug)]
pub struct CapturedFile {
	file: Mutex<File>,
	stamp: FileStamp,
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
	len: u64,
	modified: SystemTime,
	#[cfg(unix)]
	change_time: (i64, i64),
}

impl FileStamp {
	fn read(metadata: &Metadata) -> io::Result<Self> {
		#[cfg(unix)]
		use std::os::unix::fs::MetadataExt;
		Ok(Self {
			len: metadata.len(),
			modified: metadata.modified()?,
			#[cfg(unix)]
			change_time: (metadata.ctime(), metadata.ctime_nsec()),
		})
	}
}

impl CapturedFile {
	/// Takes ownership of an opened regular file. Callers choose how to resolve
	/// names, reject symlinks, and compare this handle to any earlier observation.
	///
	/// # Errors
	/// Returns an error for non-regular files or unavailable metadata.
	pub fn new(file: File) -> Result<Self, SourceError> {
		let metadata = file.metadata()?;
		if !metadata.is_file() {
			return Err(SourceError::NotRegularFile);
		}
		Ok(Self {
			file: Mutex::new(file),
			stamp: FileStamp::read(&metadata)?,
		})
	}

	/// The file length at admission.
	#[must_use]
	pub const fn len(&self) -> u64 {
		self.stamp.len
	}

	/// Whether the admitted file was empty.
	#[must_use]
	pub const fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Checks the current metadata against the admitted length and timestamps.
	///
	/// # Errors
	/// Returns an error if metadata changed or cannot be read.
	pub fn validate(&self) -> Result<(), SourceError> {
		let file = self.lock_file()?;
		self.check_stamp(&file)
	}

	/// Reads one exact byte range. Empty reads at the admitted end are valid.
	///
	/// # Errors
	/// Returns an error for ranges outside the admitted file, changed metadata,
	/// failed I/O, or short reads. A caller must discard the buffer on failure.
	pub fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), SourceError> {
		let len = buffer.len() as u64;
		if offset.checked_add(len).is_none_or(|end| end > self.len()) {
			return Err(SourceError::OutOfRange {
				offset,
				len,
				file_len: self.len(),
			});
		}
		let mut file = self.lock_file()?;
		self.check_stamp(&file)?;
		file.seek(SeekFrom::Start(offset))?;
		let result = file.read_exact(buffer);
		self.check_stamp(&file)?;
		Ok(result?)
	}

	/// Opens a logical sequential reader starting at byte zero.
	#[must_use]
	pub fn reader(&self) -> impl Read + '_ {
		CapturedReader {
			source: self,
			offset: 0,
		}
	}

	fn lock_file(&self) -> io::Result<MutexGuard<'_, File>> {
		self.file
			.lock()
			.map_err(|_| io::Error::other("file source lock poisoned"))
	}

	fn check_stamp(&self, file: &File) -> Result<(), SourceError> {
		if self.stamp != FileStamp::read(&file.metadata()?)? {
			return Err(SourceError::Changed);
		}
		Ok(())
	}
}

struct CapturedReader<'source> {
	source: &'source CapturedFile,
	offset: u64,
}

impl Read for CapturedReader<'_> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let len = buffer.len().min(
			(self.source.len() - self.offset)
				.try_into()
				.unwrap_or(usize::MAX),
		);
		self.source
			.read_exact_at(self.offset, &mut buffer[..len])
			.map_err(|error| match error {
				SourceError::Io(error) => error,
				error => io::Error::other(error),
			})?;
		self.offset += len as u64;
		Ok(len)
	}
}
