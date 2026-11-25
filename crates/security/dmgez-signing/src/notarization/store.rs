//! Durable storage in a stable trusted directory on the local filesystem.

use std::{
	ffi::OsString,
	fs::{self, File},
	io::{self, Read},
	os::unix::ffi::OsStrExt,
	path::{Path, PathBuf},
	thread,
	time::Duration,
};

use dmgez_core::output::replace::{EntryIdentity, ReplaceError, publish_replace};
use rustix::{
	fd::OwnedFd,
	fs::{self as unix, AtFlags, FileType, Mode, OFlags, Stat},
	io::Errno,
	process::geteuid,
};
use thiserror::Error;

use super::{CallError, Report};
use crate::native::notary_tool;

#[cfg(test)]
mod tests;

const MAX_RECORD_BYTES: u64 = 64 * 1024;
const LOCAL_LOCK_RETRIES: u8 = 30;
const LOCAL_LOCK_PAUSE: Duration = Duration::from_millis(10);
const PATH_SEPARATOR: u8 = b'/';
const CURRENT_DIRECTORY_NAME: &[u8] = b".";
const PARENT_DIRECTORY_NAME: &[u8] = b"..";
const CURRENT_DIRECTORY: &str = ".";
const RECORD_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const RECORD_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);

#[derive(Debug, Error)]
pub(super) enum Error {
	#[error("notarization record must name a file inside an existing local directory")]
	Path,
	#[error("notarization record already exists")]
	Exists,
	#[error("notarization record must be a regular owner-only file with mode 0600")]
	PrivateFile,
	#[error("the notarization record and its directory must have no access-control lists")]
	AccessControl,
	#[error("could not verify notarization record access controls")]
	PrivacyCheck(#[source] CallError),
	#[error("notarization record or its directory changed during the operation")]
	Changed,
	#[error("notarization record exceeds the size limit")]
	Size,
	#[error("could not access notarization record")]
	Io(#[source] io::Error),
	#[error("could not decode notarization record")]
	Decode,
	#[error("invalid notarization record")]
	Invalid,
	#[error("could not encode notarization record")]
	Encode(#[source] serde_json::Error),
	#[error("could not atomically publish notarization record")]
	Publish(#[source] Box<ReplaceError<Error>>),
	#[error("could not sync notarization record directory after publication")]
	DirectorySync(#[source] io::Error),
}

/// Holds a snapshot of the reserved record so a stale status fetch cannot
/// replace a report published by another invocation.
pub(super) struct Store {
	location: Location,
	current: Snapshot,
}

impl Store {
	pub(super) fn create(path: &Path, report: &Report) -> Result<Self, Error> {
		let location = Location::open(path)?;
		let current = location.publish(None, &encode(report)?)?;
		Ok(Self { location, current })
	}

	pub(super) fn open(path: &Path) -> Result<(Self, Report), Error> {
		let location = Location::open(path)?;
		let current = location.read()?;
		let report: Report = serde_json::from_slice(&current.bytes).map_err(|_| Error::Decode)?;
		if !report.valid_record() {
			return Err(Error::Invalid);
		}
		Ok((Self { location, current }, report))
	}

	pub(super) fn replace(&mut self, report: &Report) -> Result<(), Error> {
		self.current = self
			.location
			.publish(Some(&self.current), &encode(report)?)?;
		Ok(())
	}
}

fn encode(report: &Report) -> Result<Vec<u8>, Error> {
	if !report.valid_record() {
		return Err(Error::Invalid);
	}
	let bytes = serde_json::to_vec(report).map_err(Error::Encode)?;
	if bytes.len() as u64 > MAX_RECORD_BYTES {
		return Err(Error::Size);
	}
	Ok(bytes)
}

struct Location {
	path: PathBuf,
	parent: PathBuf,
	directory: OwnedFd,
	directory_id: EntryIdentity,
	name: OsString,
}

struct Snapshot {
	// Keeping this descriptor live prevents reuse of the observed inode.
	_file: File,
	stamp: Stat,
	bytes: Vec<u8>,
}

impl Location {
	fn open(path: &Path) -> Result<Self, Error> {
		if matches!(
			path.as_os_str()
				.as_bytes()
				.rsplit(|&byte| byte == PATH_SEPARATOR)
				.next(),
			Some(b"" | CURRENT_DIRECTORY_NAME | PARENT_DIRECTORY_NAME) | None
		) {
			return Err(Error::Path);
		}
		let name = path.file_name().ok_or(Error::Path)?.to_os_string();
		let parent = path
			.parent()
			.filter(|parent| !parent.as_os_str().is_empty())
			.unwrap_or_else(|| Path::new(CURRENT_DIRECTORY));
		let parent = fs::canonicalize(parent).map_err(Error::Io)?;
		let directory = unix::open(&parent, DIRECTORY_FLAGS, Mode::empty()).map_err(os_error)?;
		let directory_id = identity(&unix::fstat(&directory).map_err(os_error)?);
		Ok(Self {
			path: parent.join(&name),
			parent,
			directory,
			directory_id,
			name,
		})
	}

	fn read(&self) -> Result<Snapshot, Error> {
		self.check_directory()?;
		check_no_acl(&self.parent)?;
		let expected = self
			.stat()?
			.ok_or_else(|| Error::Io(io::ErrorKind::NotFound.into()))?;
		check_private(&expected)?;
		check_no_acl(&self.path)?;
		let fd = unix::openat(&self.directory, &self.name, RECORD_FLAGS, Mode::empty())
			.map_err(os_error)?;
		let mut file = File::from(fd);
		let before = unix::fstat(&file).map_err(os_error)?;
		if !same_stamp(&expected, &before) {
			return Err(Error::Changed);
		}
		let size = u64::try_from(before.st_size).map_err(|_| Error::Size)?;
		if size > MAX_RECORD_BYTES {
			return Err(Error::Size);
		}
		let mut bytes = Vec::new();
		(&mut file)
			.take(MAX_RECORD_BYTES + 1)
			.read_to_end(&mut bytes)
			.map_err(Error::Io)?;
		if bytes.len() as u64 != size
			|| !self
				.stat()?
				.is_some_and(|current| same_stamp(&before, &current))
		{
			return Err(Error::Changed);
		}
		self.check_directory()?;
		Ok(Snapshot {
			_file: file,
			stamp: before,
			bytes,
		})
	}

	fn publish(&self, expected: Option<&Snapshot>, bytes: &[u8]) -> Result<Snapshot, Error> {
		if expected.is_none() && self.stat()?.is_some() {
			return Err(Error::Exists);
		}
		let mut retries = 0;
		loop {
			let result = publish_replace(
				&self.path,
				|target| {
					if target.directory != self.directory_id {
						return Err(Error::Changed);
					}
					let Some(expected) = expected else {
						return match target.existing {
							None => check_no_acl(&self.parent),
							Some(_) => Err(Error::Exists),
						};
					};
					if target.existing != Some(identity(&expected.stamp)) {
						return Err(Error::Changed);
					}
					let current = self.read()?;
					if current.bytes != expected.bytes
						|| !same_stamp(&expected.stamp, &current.stamp)
					{
						return Err(Error::Changed);
					}
					Ok(())
				},
				|writer| writer.write_all(bytes).map_err(Error::Io),
				|_, &()| Ok(()),
			);
			match result {
				Err(ReplaceError::Busy) if retries < LOCAL_LOCK_RETRIES => {
					// Busy is returned before any write. Recheck the original
					// directory and record on the next acquisition.
					retries += 1;
					thread::sleep(LOCAL_LOCK_PAUSE);
				}
				other => {
					other.map_err(|error| match error {
						ReplaceError::Authorize(error) => error,
						ReplaceError::DestinationChanged | ReplaceError::DirectoryChanged => {
							Error::Changed
						}
						other => Error::Publish(Box::new(other)),
					})?;
					break;
				}
			}
		}
		unix::fsync(&self.directory).map_err(|error| Error::DirectorySync(error.into()))?;
		let current = self.read()?;
		if current.bytes != bytes {
			return Err(Error::Changed);
		}
		Ok(current)
	}

	fn stat(&self) -> Result<Option<Stat>, Error> {
		match unix::statat(&self.directory, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
			Ok(stat) => Ok(Some(stat)),
			Err(Errno::NOENT) => Ok(None),
			Err(error) => Err(os_error(error)),
		}
	}

	fn check_directory(&self) -> Result<(), Error> {
		let current = match unix::open(&self.parent, DIRECTORY_FLAGS, Mode::empty()) {
			Ok(directory) => unix::fstat(directory).map_err(os_error)?,
			Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP) => return Err(Error::Changed),
			Err(error) => return Err(os_error(error)),
		};
		if identity(&current) != self.directory_id {
			return Err(Error::Changed);
		}
		Ok(())
	}
}

fn check_private(stat: &Stat) -> Result<(), Error> {
	if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
		|| Mode::from_raw_mode(stat.st_mode) != RECORD_MODE
		|| stat.st_uid != geteuid().as_raw()
		|| stat.st_nlink != 1
	{
		return Err(Error::PrivateFile);
	}
	Ok(())
}

fn check_no_acl(path: &Path) -> Result<(), Error> {
	if notary_tool::has_acl(path).map_err(Error::PrivacyCheck)? {
		return Err(Error::AccessControl);
	}
	Ok(())
}

fn identity(stat: &Stat) -> EntryIdentity {
	#[cfg(target_os = "macos")]
	let device = u64::from(stat.st_dev.cast_unsigned());
	#[cfg(target_os = "linux")]
	let device = stat.st_dev;
	EntryIdentity {
		device,
		inode: stat.st_ino,
	}
}

fn same_stamp(left: &Stat, right: &Stat) -> bool {
	identity(left) == identity(right)
		&& left.st_mode == right.st_mode
		&& left.st_uid == right.st_uid
		&& left.st_size == right.st_size
		&& left.st_nlink == right.st_nlink
		&& (left.st_mtime, left.st_mtime_nsec) == (right.st_mtime, right.st_mtime_nsec)
		&& (left.st_ctime, left.st_ctime_nsec) == (right.st_ctime, right.st_ctime_nsec)
}

fn os_error(error: Errno) -> Error {
	Error::Io(error.into())
}
