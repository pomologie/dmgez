use std::{
	ffi::OsStr,
	io,
	path::{Component, Path, PathBuf},
};

use rustix::{
	fd::OwnedFd,
	fs::{self, AtFlags, Mode, OFlags, Stat},
	io::Errno,
};
use thiserror::Error;

use super::super::{DEFAULT_MAX_DEPTH, HostName};

const ROOT_DIRECTORY: &str = "/";
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const PRIVATE_DIRECTORY: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Identity {
	device: u64,
	inode: u64,
}

pub(super) fn identity(stat: &Stat) -> Identity {
	#[cfg(target_os = "macos")]
	let device = u64::from(stat.st_dev.cast_unsigned());
	#[cfg(target_os = "linux")]
	let device = stat.st_dev;
	Identity {
		device,
		inode: stat.st_ino,
	}
}

/// An explicitly authorized, pinned destination in a stable local namespace.
///
/// The caller must control the root and its containing namespace for the
/// operation's duration. The authority covers fresh output in this directory.
/// A pinned descriptor prevents substitution of a different object during
/// a relative operation. POSIX does not combine the path attachment check
/// with a later create, rename, or unlink.
#[derive(Debug)]
pub struct StableDestination {
	path: PathBuf,
	id: Identity,
	fd: OwnedFd,
}

impl StableDestination {
	/// Opens the authorized directory without following any path component.
	///
	/// # Errors
	/// Rejects a relative path, parent component, symbolic link, or an
	/// unavailable directory.
	pub fn open(path: &Path) -> Result<Self, HostError> {
		if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
			return Err(HostError::RootPath);
		}
		let fd = open_absolute(path).map_err(|error| HostError::OpenRoot(error.into()))?;
		let id = stat_fd(&fd).map_err(inspect)?;
		Ok(Self {
			path: path.into(),
			id,
			fd,
		})
	}

	/// Borrows a capability for the authorized root directory.
	#[must_use]
	pub const fn directory(&self) -> DestinationDirectory<'_> {
		DestinationDirectory {
			root: self,
			chain: Vec::new(),
			fd: None,
		}
	}
}

#[derive(Clone, Debug)]
struct Step {
	name: HostName,
	id: Identity,
}

/// A scoped directory capability for the authorized root or a freshly made child.
#[derive(Debug)]
pub struct DestinationDirectory<'root> {
	root: &'root StableDestination,
	chain: Vec<Step>,
	fd: Option<OwnedFd>,
}

/// Destination admission, identity, staging, or publication failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HostError {
	#[error("extraction root must be an absolute path without parent components")]
	RootPath,
	#[error("extraction directory depth exceeds the host limit")]
	Depth,
	#[error("could not open extraction root without following links")]
	OpenRoot(#[source] io::Error),
	#[error("destination directory moved or was replaced")]
	Detached,
	#[error("destination child crossed a filesystem boundary")]
	MountBoundary,
	#[error("destination entry already exists")]
	Conflict,
	#[error("could not inspect destination identity")]
	Inspect(#[source] io::Error),
	#[error("could not create destination directory")]
	CreateDirectory(#[source] io::Error),
	#[error("created destination directory could not be admitted")]
	CreatedDirectoryUnavailable {
		#[source]
		source: Box<HostError>,
	},
	#[error("could not create extraction staging file")]
	CreateStage(#[source] io::Error),
	#[error("staging file identity, size, or permissions changed")]
	StageChanged,
	#[error("the destination filesystem does not support the no-clobber rename")]
	NoClobberUnavailable(#[source] io::Error),
	#[error("could not publish the staged extraction file")]
	Commit(#[source] io::Error),
}

impl<'root> DestinationDirectory<'root> {
	/// Makes an owner-only child directory for subsequent extraction steps.
	/// The caller owns cleanup of created directories on a later failure.
	///
	/// # Errors
	/// Reports existing names, detected namespace changes, depth limits
	/// and filesystem failures. [`HostError::CreatedDirectoryUnavailable`]
	/// indicates creation succeeded before attachment could be confirmed.
	pub fn create_child(&self, name: &HostName) -> Result<Self, HostError> {
		self.create_child_with(name, || self.open_created_child(name))
	}

	pub(super) fn create_child_with(
		&self,
		name: &HostName,
		open_created: impl FnOnce() -> Result<Self, HostError>,
	) -> Result<Self, HostError> {
		if self.chain.len() >= DEFAULT_MAX_DEPTH {
			return Err(HostError::Depth);
		}
		let name_path = name.as_str();
		let parent = self.descriptor();
		self.check_attachment()?;
		fs::mkdirat(parent, name_path, PRIVATE_DIRECTORY).map_err(|error| match error {
			Errno::EXIST => HostError::Conflict,
			_ => HostError::CreateDirectory(error.into()),
		})?;
		open_created().map_err(|source| HostError::CreatedDirectoryUnavailable {
			source: Box::new(source),
		})
	}

	fn open_created_child(&self, name: &HostName) -> Result<Self, HostError> {
		let name_path = name.as_str();
		let parent = self.descriptor();
		let observed =
			fs::statat(parent, name_path, AtFlags::SYMLINK_NOFOLLOW).map_err(namespace_error)?;
		let fd = fs::openat(parent, name_path, DIRECTORY_FLAGS, Mode::empty())
			.map_err(namespace_error)?;
		let id = stat_fd(&fd).map_err(inspect)?;
		if id.device != self.root.id.device {
			return Err(HostError::MountBoundary);
		}
		if id != identity(&observed) {
			return Err(HostError::Detached);
		}
		let mut chain = self.chain.clone();
		chain.push(Step {
			name: name.clone(),
			id,
		});
		let child = Self {
			root: self.root,
			chain,
			fd: Some(fd),
		};
		child.check_attachment()?;
		Ok(child)
	}

	/// Resolves the authorized attachment again and verifies every identity.
	///
	/// # Errors
	/// Reports a moved or replaced namespace or an inspection failure.
	pub fn check_attachment(&self) -> Result<(), HostError> {
		let mut current = open_absolute(&self.root.path).map_err(namespace_error)?;
		if stat_fd(&current).map_err(inspect)? != self.root.id {
			return Err(HostError::Detached);
		}
		for step in &self.chain {
			current = fs::openat(current, step.name.as_str(), DIRECTORY_FLAGS, Mode::empty())
				.map_err(namespace_error)?;
			if stat_fd(&current).map_err(inspect)? != step.id {
				return Err(HostError::Detached);
			}
		}
		Ok(())
	}

	pub(super) fn descriptor(&self) -> &OwnedFd {
		self.fd.as_ref().unwrap_or(&self.root.fd)
	}

	pub(super) fn root_path(&self) -> &Path {
		&self.root.path
	}

	/// Checks that the admitted destination name is absent without following links.
	///
	/// This check leaves the namespace untouched and does not reserve the name.
	///
	/// # Errors
	/// Reports a detached directory, an existing entry, or an inspection failure.
	pub fn require_absent(&self, name: &HostName) -> Result<(), HostError> {
		self.check_attachment()?;
		if stat_entry(self.descriptor(), name.as_str().as_ref())
			.map_err(inspect)?
			.is_some()
		{
			return Err(HostError::Conflict);
		}
		Ok(())
	}
}

fn open_absolute(path: &Path) -> rustix::io::Result<OwnedFd> {
	let mut cursor = fs::open(ROOT_DIRECTORY, DIRECTORY_FLAGS, Mode::empty())?;
	for component in path.components() {
		if component != Component::RootDir {
			cursor = fs::openat(
				cursor,
				component.as_os_str(),
				DIRECTORY_FLAGS,
				Mode::empty(),
			)?;
		}
	}
	Ok(cursor)
}

fn namespace_error(error: Errno) -> HostError {
	match error {
		Errno::LOOP | Errno::NOENT | Errno::NOTDIR | Errno::STALE => HostError::Detached,
		_ => inspect(error),
	}
}

pub(super) fn inspect(error: Errno) -> HostError {
	HostError::Inspect(error.into())
}

fn stat_fd(fd: &OwnedFd) -> rustix::io::Result<Identity> {
	fs::fstat(fd).map(|stat| identity(&stat))
}

pub(super) fn stat_entry(fd: &OwnedFd, name: &OsStr) -> rustix::io::Result<Option<Stat>> {
	match fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) => Ok(Some(stat)),
		Err(Errno::NOENT) => Ok(None),
		Err(error) => Err(error),
	}
}
