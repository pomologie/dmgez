//! Descriptor-relative replacement for trusted local macOS and Linux directories.

use std::{
	ffi::{OsStr, OsString},
	fs::File,
	io::{self, Write},
	os::unix::ffi::OsStrExt,
	path::{Component, Path, absolute},
};

use rustix::{
	fd::OwnedFd,
	fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags, RenameFlags, Stat},
	io::Errno,
};
use tempfile::Builder;
use thiserror::Error;

use super::{PublishError, StagedReader, prepare_stage};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const STAGE_FLAGS: OFlags = OFlags::RDWR
	.union(OFlags::CREATE)
	.union(OFlags::EXCL)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const STAGE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const DIRECTORY_ROOT: &str = "/";
const CURRENT_DIRECTORY: &[u8] = b".";
const PARENT_DIRECTORY: &[u8] = b"..";
const PATH_SEPARATOR: u8 = b'/';
const STAGE_PREFIX_ZERO: &str = ".dmgez-stage-0-";
const STAGE_PREFIX_ONE: &str = ".dmgez-stage-1-";
const ZERO_DIGIT: u8 = b'0';

/// A Unix filesystem object identity, as reported by `stat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryIdentity {
	pub device: u64,
	pub inode: u64,
}

/// The identities checked by a caller's source-alias policy before staging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetIdentity {
	pub directory: EntryIdentity,
	pub existing: Option<EntryIdentity>,
}

/// A failed publication under the authorized replacement policy.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReplaceError<E> {
	#[error("destination must end in a file name")]
	InvalidPath,
	#[error("could not open the output directory without following symbolic links")]
	Directory(#[source] io::Error),
	#[error("another publisher holds the output directory lock")]
	Busy,
	#[error("could not lock the output directory")]
	Lock(#[source] io::Error),
	#[error("could not inspect the output or staging entry")]
	Inspect(#[source] io::Error),
	#[error("replacement destination must be a regular file or absent")]
	DestinationType,
	#[error("replacement destination changed before commit")]
	DestinationChanged,
	#[error("output directory changed before commit")]
	DirectoryChanged,
	#[error("staging entry changed before commit")]
	StageChanged,
	#[error("replacement did not pass the source-alias check")]
	Authorize(#[source] E),
	#[error(transparent)]
	Stage(#[from] PublishError<E>),
	#[error("could not atomically publish the staged output")]
	Commit(#[source] io::Error),
}

/// Publishes a synced and validated stage under an explicit overwrite policy.
///
/// `authorize` must reject identities that match any protected source entry
/// or input directory. The directory is held by descriptor. It and the
/// no-follow target fingerprint are checked again before commit.
///
/// Use a trusted local directory whose publishers cooperate with its advisory
/// lock. POSIX offers no conditional replacement by inode. A noncooperating
/// namespace mutation between the final check and the rename is outside the
/// contract. An initially absent destination uses the native no-clobber
/// operation even though the caller authorized replacement. The function
/// syncs the stage contents. Directory crash durability requires a separate
/// parent-directory sync.
///
/// [POSIX rename](https://pubs.opengroup.org/onlinepubs/9799919799/functions/rename.html)
/// and [advisory locking](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html).
///
/// # Errors
/// Returns before publication on a failed admission, write, sync, validation,
/// lock, identity recheck, or rename. The validation callback can report
/// cancellation. A failed kernel replacement leaves the existing entry intact
/// under the local filesystem's atomic rename contract.
pub fn publish_replace<T, E>(
	destination: &Path,
	authorize: impl FnOnce(TargetIdentity) -> Result<(), E>,
	write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
	validate: impl FnOnce(StagedReader<'_>, &T) -> Result<(), E>,
) -> Result<T, ReplaceError<E>> {
	let path_bytes = destination.as_os_str().as_bytes();
	match path_bytes.rsplit(|&byte| byte == PATH_SEPARATOR).next() {
		Some(b"" | CURRENT_DIRECTORY | PARENT_DIRECTORY) | None => {
			return Err(ReplaceError::InvalidPath);
		}
		_ => {}
	}
	let destination = absolute(destination).map_err(ReplaceError::Directory)?;
	let (Some(parent), Some(name)) = (destination.parent(), destination.file_name()) else {
		return Err(ReplaceError::InvalidPath);
	};
	let directory =
		open_directory(parent).map_err(|error| ReplaceError::Directory(error.into()))?;
	match fs::flock(&directory, FlockOperation::NonBlockingLockExclusive) {
		Ok(()) => {}
		Err(Errno::WOULDBLOCK) => return Err(ReplaceError::Busy),
		Err(error) => return Err(ReplaceError::Lock(error.into())),
	}
	let directory_id = fs::fstat(&directory)
		.map(|stat| identity(&stat))
		.map_err(inspect)?;
	let original = stat_entry(&directory, name).map_err(inspect)?;
	if original.as_ref().is_some_and(|stat| !is_regular(stat)) {
		return Err(ReplaceError::DestinationType);
	}
	authorize(TargetIdentity {
		directory: directory_id,
		existing: original.as_ref().map(identity),
	})
	.map_err(ReplaceError::Authorize)?;

	let mut stage = Stage::new(&directory, parent, name).map_err(PublishError::Create)?;
	let report = prepare_stage(&mut stage.file, write)?;
	let stage_check = fs::fstat(&stage.file).map_err(inspect)?;
	validate(StagedReader(&mut stage.file), &report).map_err(PublishError::Validate)?;
	let stage_current = stat_entry(&directory, &stage.name).map_err(inspect)?;
	if !stage_current
		.as_ref()
		.is_some_and(|current| matches_stamp(&stage_check, current))
	{
		return Err(ReplaceError::StageChanged);
	}
	let current_directory = match open_directory(parent) {
		Ok(directory) => fs::fstat(directory)
			.map(|stat| identity(&stat))
			.map_err(inspect)?,
		Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP) => {
			return Err(ReplaceError::DirectoryChanged);
		}
		Err(error) => return Err(ReplaceError::Directory(error.into())),
	};
	if current_directory != directory_id {
		return Err(ReplaceError::DirectoryChanged);
	}
	let current = stat_entry(&directory, name).map_err(inspect)?;
	match (original, current) {
		(Some(original), Some(current)) if matches_stamp(&original, &current) => {
			fs::renameat(&directory, &stage.name, &directory, name)
		}
		(None, None) => fs::renameat_with(
			&directory,
			&stage.name,
			&directory,
			name,
			RenameFlags::NOREPLACE,
		),
		_ => return Err(ReplaceError::DestinationChanged),
	}
	.map_err(|error| ReplaceError::Commit(error.into()))?;
	stage.committed = true;
	Ok(report)
}

fn open_directory(path: &Path) -> rustix::io::Result<OwnedFd> {
	let mut directory = fs::open(DIRECTORY_ROOT, DIRECTORY_FLAGS, Mode::empty())?;
	for component in path.components() {
		if component != Component::RootDir {
			directory = fs::openat(
				directory,
				component.as_os_str(),
				DIRECTORY_FLAGS,
				Mode::empty(),
			)?;
		}
	}
	Ok(directory)
}

fn stat_entry(directory: &OwnedFd, name: &OsStr) -> rustix::io::Result<Option<Stat>> {
	match fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) => Ok(Some(stat)),
		Err(Errno::NOENT) => Ok(None),
		Err(error) => Err(error),
	}
}

fn inspect<E>(error: Errno) -> ReplaceError<E> {
	ReplaceError::Inspect(error.into())
}

fn is_regular(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
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

fn matches_stamp(expected: &Stat, current: &Stat) -> bool {
	identity(expected) == identity(current)
		&& expected.st_mode == current.st_mode
		&& expected.st_size == current.st_size
		&& expected.st_nlink == current.st_nlink
		&& (expected.st_mtime, expected.st_mtime_nsec) == (current.st_mtime, current.st_mtime_nsec)
		&& (expected.st_ctime, expected.st_ctime_nsec) == (current.st_ctime, current.st_ctime_nsec)
}

struct Stage<'directory> {
	directory: &'directory OwnedFd,
	name: OsString,
	file: File,
	committed: bool,
}

impl<'directory> Stage<'directory> {
	fn new(directory: &'directory OwnedFd, parent: &Path, destination: &OsStr) -> io::Result<Self> {
		// A distinct first ASCII digit prevents the candidate from becoming the
		// destination under the supported filesystems' case or canonical folding.
		let prefix = match destination
			.as_bytes()
			.iter()
			.copied()
			.find(u8::is_ascii_digit)
		{
			Some(ZERO_DIGIT) => STAGE_PREFIX_ONE,
			_ => STAGE_PREFIX_ZERO,
		};
		let named = Builder::new()
			.prefix(prefix)
			.disable_cleanup(true)
			.make_in(parent, |path| {
				let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
				let file = fs::openat(directory, name, STAGE_FLAGS, STAGE_MODE)?;
				Ok((File::from(file), name.to_os_string()))
			})?;
		let ((file, name), _) = named.into_parts();
		Ok(Self {
			directory,
			name,
			file,
			committed: false,
		})
	}
}

impl Drop for Stage<'_> {
	fn drop(&mut self) {
		if !self.committed
			&& let Ok(original) = fs::fstat(&self.file)
			&& let Ok(Some(current)) = stat_entry(self.directory, &self.name)
			&& identity(&original) == identity(&current)
		{
			let _ = fs::unlinkat(self.directory, &self.name, AtFlags::empty());
		}
	}
}
