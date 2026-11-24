use std::{
	ffi::OsStr,
	fs::File,
	io,
	os::unix::ffi::OsStrExt,
	path::{Component, Path, PathBuf},
	sync::Arc,
};

use rustix::{
	fd::{AsFd, OwnedFd},
	fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat},
	io::Errno,
};

use super::{
	Budget, Error, MAX_SYMLINK_BYTES, SourceIdentity, SourceMetadata, SourceSnapshot, Timestamp,
};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const SOURCE_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::NOCTTY)
	.union(OFlags::CLOEXEC);
const DIRECTORY_SELF: &str = ".";
const DIRECTORY_PARENT: &str = "..";
const MAX_NANOSECONDS: u32 = 1_000_000_000 - 1;
const STAT_SOURCE: &str = "stat source";
const STAT_OUTPUT: &str = "stat output";
const OPEN_PARENT: &str = "open directory without following symlinks";
const OPEN_SOURCE: &str = "open regular source";
const READ_DIRECTORY: &str = "read source directory";
const READ_LINK: &str = "read source link";

pub(super) struct Anchor {
	root: Arc<OwnedFd>,
}

impl Anchor {
	pub(super) fn new() -> Result<Self, Error> {
		let path = Path::new("/");
		let root = fs::open(path, DIRECTORY_FLAGS, Mode::empty())
			.map_err(|err| annotated(OPEN_PARENT, path, err))?;
		Ok(Self {
			root: Arc::new(root),
		})
	}

	pub(super) fn locate<'a>(&self, path: &'a Path) -> Result<Leaf<'a>, Error> {
		let (directory, name) =
			resolve_parent(&self.root, path).map_err(|err| annotated(OPEN_PARENT, path, err))?;
		Ok(Leaf { directory, name })
	}

	pub(super) fn snapshot(
		&self,
		directory: impl AsFd,
		name: &OsStr,
		path: PathBuf,
	) -> Result<SourceSnapshot, Error> {
		let stat = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|err| annotated(STAT_SOURCE, &path, err))?;
		let metadata = metadata(&stat).map_err(|err| annotated(STAT_SOURCE, &path, err))?;
		Ok(SourceSnapshot {
			root: Arc::clone(&self.root),
			kind: FileType::from_raw_mode(stat.st_mode),
			path,
			metadata,
		})
	}

	pub(super) fn probe_output(&self, path: &Path) -> Result<OutputProbe, Error> {
		let (directory, name) = match resolve_parent(&self.root, path) {
			Ok(location) => location,
			Err(Errno::NOENT) => return Ok(OutputProbe::default()),
			Err(err) => return Err(annotated(OPEN_PARENT, path, err)),
		};
		let parent = fs::fstat(&directory).map_err(|err| annotated(STAT_OUTPUT, path, err))?;
		let output = match fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
			Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => {
				Some(identity(&stat).map_err(|err| annotated(STAT_OUTPUT, path, err))?)
			}
			Ok(_) => return Err(Error::UnsupportedOutput { path: path.into() }),
			Err(Errno::NOENT) => None,
			Err(err) => return Err(annotated(STAT_OUTPUT, path, err)),
		};
		Ok(OutputProbe {
			parent: Some(identity(&parent).map_err(|err| annotated(STAT_OUTPUT, path, err))?),
			output,
		})
	}
}

pub(super) struct Leaf<'a> {
	pub(super) directory: OwnedFd,
	pub(super) name: &'a OsStr,
}

#[derive(Default)]
pub(super) struct OutputProbe {
	parent: Option<SourceIdentity>,
	output: Option<SourceIdentity>,
}

impl OutputProbe {
	pub(super) fn check(&self, source: &SourceSnapshot) -> Result<(), Error> {
		let identity = Some(source.metadata.identity);
		if self.output == identity
			|| (source.kind == FileType::Directory && self.parent == identity)
		{
			return Err(Error::OutputSourceAlias {
				path: source.path.clone(),
			});
		}
		Ok(())
	}
}

pub(super) fn open_directory(
	parent: impl AsFd,
	name: &OsStr,
	source: &SourceSnapshot,
) -> Result<Dir, Error> {
	let directory = fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
		.map_err(|err| recheck_error(source, OPEN_PARENT, err))?;
	match_opened(source, &directory)?;
	Dir::new(directory).map_err(|err| annotated(READ_DIRECTORY, &source.path, err))
}

pub(super) fn directory_handle<'a>(
	directory: &'a Dir,
	source: &SourceSnapshot,
) -> Result<rustix::fd::BorrowedFd<'a>, Error> {
	directory
		.fd()
		.map_err(|err| annotated(READ_DIRECTORY, &source.path, err))
}

pub(super) fn next_name(
	directory: &mut Dir,
	source: &SourceSnapshot,
) -> Result<Option<std::ffi::OsString>, Error> {
	for next in directory {
		let entry = next.map_err(|err| annotated(READ_DIRECTORY, &source.path, err))?;
		let name = OsStr::from_bytes(entry.file_name().to_bytes());
		if name != DIRECTORY_SELF && name != DIRECTORY_PARENT {
			return Ok(Some(name.to_owned()));
		}
	}
	Ok(None)
}

pub(super) fn read_link(
	parent: impl AsFd,
	name: &OsStr,
	source: &SourceSnapshot,
) -> Result<Vec<u8>, Error> {
	let mut bytes = [0_u8; MAX_SYMLINK_BYTES + 1];
	let length = fs::readlinkat_raw(&parent, name, &mut bytes[..])
		.map_err(|err| recheck_error(source, READ_LINK, err))?;
	if length > MAX_SYMLINK_BYTES {
		return Err(Error::Limit(Budget::Symlink));
	}
	let observed = fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
		.map_err(|err| recheck_error(source, STAT_SOURCE, err))?;
	match_stat(source, &observed)?;
	Ok(bytes[..length].to_vec())
}

pub(super) fn open_regular(source: &SourceSnapshot) -> Result<File, Error> {
	if source.kind != FileType::RegularFile {
		return Err(Error::RegularSourceRequired {
			path: source.path.clone(),
		});
	}
	let (directory, name) = resolve_parent(&source.root, &source.path)
		.map_err(|err| recheck_error(source, OPEN_PARENT, err))?;
	let observed = fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
		.map_err(|err| recheck_error(source, STAT_SOURCE, err))?;
	match_stat(source, &observed)?;
	let file = fs::openat(directory, name, SOURCE_FLAGS, Mode::empty())
		.map_err(|err| recheck_error(source, OPEN_SOURCE, err))?;
	match_opened(source, &file)?;
	Ok(File::from(file))
}

pub(super) fn validate(source: &SourceSnapshot) -> Result<(), Error> {
	let (directory, name) = resolve_parent(&source.root, &source.path)
		.map_err(|err| recheck_error(source, OPEN_PARENT, err))?;
	let stat = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
		.map_err(|err| recheck_error(source, STAT_SOURCE, err))?;
	match_stat(source, &stat)
}

pub(super) fn match_opened(source: &SourceSnapshot, file: impl AsFd) -> Result<(), Error> {
	let stat = fs::fstat(file).map_err(|err| recheck_error(source, STAT_SOURCE, err))?;
	match_stat(source, &stat)
}

fn match_stat(source: &SourceSnapshot, stat: &Stat) -> Result<(), Error> {
	let mut observed = metadata(stat).map_err(|err| annotated(STAT_SOURCE, &source.path, err))?;
	// Reading an admitted file or directory may legitimately update its access time.
	observed.accessed = source.metadata.accessed;
	if FileType::from_raw_mode(stat.st_mode) != source.kind || observed != source.metadata {
		return Err(Error::SourceChanged {
			path: source.path.clone(),
		});
	}
	Ok(())
}

fn resolve_parent<'a>(root: &OwnedFd, path: &'a Path) -> rustix::io::Result<(OwnedFd, &'a OsStr)> {
	let mut components = path.components();
	if components.next() != Some(Component::RootDir) {
		return Err(Errno::INVAL);
	}
	let name = components
		.next_back()
		.map_or(OsStr::new(DIRECTORY_SELF), |part| part.as_os_str());
	let mut directory = rustix::io::fcntl_dupfd_cloexec(root, 0)?;
	for component in components {
		directory = fs::openat(
			&directory,
			component.as_os_str(),
			DIRECTORY_FLAGS,
			Mode::empty(),
		)?;
	}
	Ok((directory, name))
}

fn annotated(operation: &'static str, path: &Path, source: impl Into<io::Error>) -> Error {
	Error::Io {
		operation,
		path: path.into(),
		source: source.into(),
	}
}

fn recheck_error(source: &SourceSnapshot, operation: &'static str, error: Errno) -> Error {
	match error {
		Errno::NOENT | Errno::NOTDIR | Errno::LOOP | Errno::STALE => Error::SourceChanged {
			path: source.path.clone(),
		},
		_ => annotated(operation, &source.path, error),
	}
}

fn metadata(stat: &Stat) -> io::Result<SourceMetadata> {
	#[cfg(target_os = "macos")]
	let (created, flags) = (
		Some(timestamp(stat.st_birthtime, stat.st_birthtime_nsec)?),
		Some(stat.st_flags),
	);
	#[cfg(not(target_os = "macos"))]
	let (created, flags) = (None, None);
	Ok(SourceMetadata {
		identity: identity(stat)?,
		size: checked_stat_value(stat.st_size)?,
		mode: checked_stat_value(Mode::from_raw_mode(stat.st_mode).bits())?,
		uid: stat.st_uid,
		gid: stat.st_gid,
		link_count: checked_stat_value(stat.st_nlink)?,
		accessed: timestamp(stat.st_atime, stat.st_atime_nsec)?,
		modified: timestamp(stat.st_mtime, stat.st_mtime_nsec)?,
		changed: timestamp(stat.st_ctime, stat.st_ctime_nsec)?,
		created,
		flags,
	})
}

fn identity(stat: &Stat) -> io::Result<SourceIdentity> {
	#[cfg(target_os = "macos")]
	let device = u64::from(stat.st_dev.cast_unsigned());
	#[cfg(not(target_os = "macos"))]
	let device = checked_stat_value(stat.st_dev)?;
	Ok(SourceIdentity {
		device,
		inode: checked_stat_value(stat.st_ino)?,
	})
}

fn timestamp(seconds: impl Into<i64>, nanoseconds: impl TryInto<u32>) -> io::Result<Timestamp> {
	let nanoseconds = checked_stat_value::<u32, _>(nanoseconds)?;
	if nanoseconds > MAX_NANOSECONDS {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"invalid stat timestamp",
		));
	}
	Ok(Timestamp {
		seconds: seconds.into(),
		nanoseconds,
	})
}

fn checked_stat_value<T, S: TryInto<T>>(value: S) -> io::Result<T> {
	value
		.try_into()
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid stat value"))
}
