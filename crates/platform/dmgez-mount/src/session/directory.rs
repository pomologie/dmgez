use std::{
	ffi::OsStr,
	path::{Component, Path, PathBuf},
};

use dmgez_core::private_directory::no_extended_acl;
use rustix::{
	fd::{AsFd, OwnedFd},
	fs::{self, AtFlags, FlockOperation, Mode, OFlags, RenameFlags, Stat},
	io::Errno,
	process::geteuid,
};

use crate::system::valid_native_path;

use super::Error;

const ROOT_PATH: &str = "/";
const PRIVATE_DIRECTORY_MODE: Mode = Mode::RWXU;
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);

pub(super) struct RootDirectory {
	path: PathBuf,
	descriptor: OwnedFd,
}

impl RootDirectory {
	pub(super) fn open(path: &Path) -> Result<Self, Error> {
		validate_path(path)?;
		let parent = open_directory(path.parent().ok_or(Error::Path)?)?;
		let name = path.file_name().ok_or(Error::Path)?;
		let created = match fs::mkdirat(&parent, name, PRIVATE_DIRECTORY_MODE) {
			Ok(()) => true,
			Err(Errno::EXIST) => false,
			Err(error) => return Err(error.into()),
		};
		let descriptor = open_private_at(&parent, name)?;
		match fs::flock(&descriptor, FlockOperation::NonBlockingLockExclusive) {
			Ok(()) => {}
			Err(Errno::WOULDBLOCK) => return Err(Error::Busy),
			Err(error) => return Err(error.into()),
		}
		let root = Self {
			path: path.to_path_buf(),
			descriptor,
		};
		root.check_binding()?;
		if created {
			fs::fsync(&parent)?;
		}
		Ok(root)
	}

	pub(super) fn path(&self) -> &Path {
		&self.path
	}

	pub(super) const fn descriptor(&self) -> &OwnedFd {
		&self.descriptor
	}

	pub(super) fn private_child(&self, name: &str) -> Result<OwnedFd, Error> {
		open_private_at(&self.descriptor, name.as_ref())
	}

	pub(super) fn mkdir(&self, name: &str) -> rustix::io::Result<()> {
		fs::mkdirat(&self.descriptor, name, PRIVATE_DIRECTORY_MODE)
	}

	pub(super) fn exists(&self, name: &str) -> Result<bool, Error> {
		match fs::statat(&self.descriptor, name, AtFlags::SYMLINK_NOFOLLOW) {
			Ok(_) => Ok(true),
			Err(Errno::NOENT) => Ok(false),
			Err(error) => Err(error.into()),
		}
	}

	pub(super) fn publish_child(
		&self,
		child: &OwnedFd,
		staged: &str,
		final_name: &str,
	) -> Result<(), Error> {
		self.check_binding()?;
		self.check_child(staged, child)?;
		fs::renameat_with(
			&self.descriptor,
			staged,
			&self.descriptor,
			final_name,
			RenameFlags::NOREPLACE,
		)?;
		fs::fsync(&self.descriptor)?;
		self.check_child(final_name, child)?;
		self.check_binding()
	}

	pub(super) fn check_child(&self, name: &str, child: &OwnedFd) -> Result<(), Error> {
		let named = self.private_child(name)?;
		if !same_entry(&fs::fstat(&named)?, &fs::fstat(child)?) {
			return Err(Error::RegistryChanged);
		}
		Ok(())
	}

	pub(super) fn check_binding(&self) -> Result<(), Error> {
		let named = open_directory(&self.path).map_err(|_| Error::RegistryChanged)?;
		if !same_entry(&fs::fstat(&named)?, &fs::fstat(&self.descriptor)?) {
			return Err(Error::RegistryChanged);
		}
		require_private(&named)
	}
}

impl Drop for RootDirectory {
	fn drop(&mut self) {
		// Unlock explicitly even if an inherited open-file description is in
		// the short pre-exec window of a different process.
		let _ = fs::flock(&self.descriptor, FlockOperation::Unlock);
	}
}

pub(super) fn validate_path(path: &Path) -> Result<(), Error> {
	valid_native_path(path).then_some(()).ok_or(Error::Path)
}

pub(super) fn create_directory(parent: impl AsFd, name: &str) -> Result<(), Error> {
	fs::mkdirat(&parent, name, PRIVATE_DIRECTORY_MODE)?;
	let directory = open_private_at(parent, name.as_ref())?;
	fs::fsync(&directory)?;
	Ok(())
}

fn open_private_at(parent: impl AsFd, name: &OsStr) -> Result<OwnedFd, Error> {
	let directory = fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| {
		if error == Errno::NOENT {
			Error::Missing
		} else {
			error.into()
		}
	})?;
	require_private(&directory)?;
	Ok(directory)
}

fn open_directory(path: &Path) -> Result<OwnedFd, Error> {
	let mut directory = fs::open(ROOT_PATH, DIRECTORY_FLAGS, Mode::empty())?;
	for part in path.components() {
		if let Component::Normal(name) = part {
			directory = fs::openat(directory, name, DIRECTORY_FLAGS, Mode::empty())?;
		}
	}
	Ok(directory)
}

fn require_private(directory: &OwnedFd) -> Result<(), Error> {
	let stat = fs::fstat(directory)?;
	if Mode::from_raw_mode(stat.st_mode) != PRIVATE_DIRECTORY_MODE
		|| stat.st_uid != geteuid().as_raw()
		|| !no_extended_acl(directory.as_fd())?
	{
		return Err(Error::Privacy);
	}
	Ok(())
}

pub(super) fn same_entry(first: &Stat, second: &Stat) -> bool {
	first.st_dev == second.st_dev && first.st_ino == second.st_ino
}
