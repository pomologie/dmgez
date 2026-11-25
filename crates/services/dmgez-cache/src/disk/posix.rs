use std::{
	io,
	num::NonZeroU64,
	path::{Component, Path},
	sync::Mutex,
};

use rustix::{
	fd::{AsFd, OwnedFd},
	fs::{self, FileType, FlockOperation, Mode, OFlags, Stat},
	process::geteuid,
};

use super::{DirectoryIdentity, Error};

#[cfg(target_os = "macos")]
mod darwin_acl;
mod entry;
#[cfg(test)]
mod tests;

const ROOT_PATH: &str = "/";
const SELF_PATH: &str = ".";
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const DIRECTORY_MODE: Mode = Mode::RWXU;
const FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const MAX_ROOT_DEPTH: usize = 1024;
// A stored zlib block is smaller than its original UDIF writer chunk.
const MAX_BLOCK_BYTES: usize = 1 << 20;

#[derive(Debug)]
pub(super) struct Cache {
	root: OwnedFd,
	ancestors: Vec<DirectoryIdentity>,
	max_bytes: NonZeroU64,
	local: Mutex<()>,
}

impl Cache {
	pub(super) fn open(path: &Path, max_bytes: NonZeroU64) -> Result<Self, Error> {
		let mut components = path.components();
		if components.next() != Some(Component::RootDir)
			|| components.as_path().as_os_str().is_empty()
		{
			return Err(Error::Path);
		}
		let mut directory =
			fs::open(ROOT_PATH, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
		let mut ancestors = vec![identity(&fs::fstat(&directory).map_err(io::Error::from)?)?];
		for component in components {
			let Component::Normal(name) = component else {
				return Err(Error::Path);
			};
			if ancestors.len() > MAX_ROOT_DEPTH {
				return Err(Error::Path);
			}
			directory = fs::openat(&directory, name, DIRECTORY_FLAGS, Mode::empty())
				.map_err(io::Error::from)?;
			ancestors.try_reserve(1).map_err(io::Error::other)?;
			ancestors.push(identity(&fs::fstat(&directory).map_err(io::Error::from)?)?);
		}
		if !private_directory(&directory)? {
			return Err(Error::Privacy);
		}
		Ok(Self {
			root: directory,
			ancestors,
			max_bytes,
			local: Mutex::new(()),
		})
	}

	pub(super) fn directory_identity(&self) -> DirectoryIdentity {
		*self
			.ancestors
			.last()
			.expect("opened cache root has an identity")
	}

	pub(super) fn ancestors(&self) -> &[DirectoryIdentity] {
		&self.ancestors
	}

	pub(super) fn load(&self, key: &[u8; 32], output: &mut [u8]) -> Option<usize> {
		self.with_lock(|directory| entry::load(directory, key, output, self.max_bytes.get()))
			.ok()
	}

	pub(super) fn store(&self, key: &[u8; 32], encoded: &[u8]) {
		if !encoded.is_empty()
			&& encoded.len() <= MAX_BLOCK_BYTES
			&& encoded.len() as u64 <= self.max_bytes.get()
		{
			let _ = self
				.with_lock(|directory| entry::store(directory, key, encoded, self.max_bytes.get()));
		}
	}

	fn with_lock<T>(&self, operation: impl FnOnce(&OwnedFd) -> io::Result<T>) -> io::Result<T> {
		let _local = self
			.local
			.lock()
			.map_err(|_| io::Error::other("cache lock poisoned"))?;
		// A new open file description gives each process its own advisory lock.
		let directory = fs::openat(&self.root, SELF_PATH, DIRECTORY_FLAGS, Mode::empty())?;
		fs::flock(&directory, FlockOperation::NonBlockingLockExclusive)?;
		let locked = LockedDirectory(directory);
		if !private_directory(&locked.0)? {
			return Err(io::ErrorKind::PermissionDenied.into());
		}
		operation(&locked.0)
	}
}

struct LockedDirectory(OwnedFd);

impl Drop for LockedDirectory {
	fn drop(&mut self) {
		// An in-flight fork can inherit the file description before CLOEXEC.
		// Release the lock even if another process briefly retains its handle.
		let _ = fs::flock(&self.0, FlockOperation::Unlock);
	}
}

fn private_directory(directory: impl AsFd) -> io::Result<bool> {
	let stat = fs::fstat(&directory)?;
	Ok(Mode::from_raw_mode(stat.st_mode) == DIRECTORY_MODE
		&& stat.st_uid == geteuid().as_raw()
		&& no_extended_acl(directory)?)
}

fn private_file(file: impl AsFd, stat: &Stat) -> io::Result<bool> {
	Ok(owned_regular_file(stat)
		&& Mode::from_raw_mode(stat.st_mode) == FILE_MODE
		&& no_extended_acl(file)?)
}

fn owned_regular_file(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
		&& stat.st_uid == geteuid().as_raw()
		&& stat.st_nlink == 1
}

fn no_extended_acl(file: impl AsFd) -> io::Result<bool> {
	#[cfg(target_os = "macos")]
	{
		darwin_acl::absent(file.as_fd())
	}
	#[cfg(target_os = "linux")]
	{
		let _ = file;
		// POSIX ACL permissions for named users and groups are limited by the
		// group class mask. The exact owner-only mode checked above allows none.
		Ok(true)
	}
}

fn identity(stat: &Stat) -> io::Result<DirectoryIdentity> {
	#[cfg(target_os = "macos")]
	let device = u64::from(stat.st_dev.cast_unsigned());
	#[cfg(target_os = "linux")]
	let device = identity_value(stat.st_dev)?;
	Ok((device, identity_value(stat.st_ino)?))
}

fn identity_value(value: impl TryInto<u64>) -> io::Result<u64> {
	value
		.try_into()
		.map_err(|_| io::ErrorKind::InvalidData.into())
}
