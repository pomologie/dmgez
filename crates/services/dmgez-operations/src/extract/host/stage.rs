use std::{
	ffi::OsString,
	fs::File,
	io::{self, Write},
	path::Path,
};

use rustix::{
	fd::OwnedFd,
	fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, Stat},
	io::Errno,
};
use tempfile::Builder;

use super::{
	HostError, HostName,
	directory::{identity, inspect, stat_entry},
};

const STAGE_FLAGS: OFlags = OFlags::WRONLY
	.union(OFlags::CREATE)
	.union(OFlags::EXCL)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);
const TEMP_RANDOM_BYTES: usize = 16;
const MAX_STAGE_ATTEMPTS: usize = 32;
const ZERO_DIGIT: u8 = b'0';
const STAGING_PREFIX_ZERO: &str = ".dmgez-extract-0-";
const STAGING_PREFIX_ONE: &str = ".dmgez-extract-1-";

/// File name and descriptor owned by one in-progress copy.
pub(super) struct Stage<'dir> {
	directory: &'dir OwnedFd,
	name: OsString,
	pub(super) file: File,
	committed: bool,
}

impl<'dir> Stage<'dir> {
	pub(super) fn create(
		directory: &'dir OwnedFd,
		path: &Path,
		target: &HostName,
	) -> Result<Self, HostError> {
		// Choose a different first digit from the admitted destination name.
		// That keeps the temporary name distinct under either name profile.
		let prefix = match target.as_str().bytes().find(u8::is_ascii_digit) {
			Some(ZERO_DIGIT) => STAGING_PREFIX_ONE,
			_ => STAGING_PREFIX_ZERO,
		};
		let mut attempts = 0;
		let named = Builder::new()
			.prefix(prefix)
			.rand_bytes(TEMP_RANDOM_BYTES)
			.disable_cleanup(true)
			.make_in(path, |candidate| {
				if attempts == MAX_STAGE_ATTEMPTS {
					return Err(io::Error::other("could not reserve a unique stage name"));
				}
				attempts += 1;
				// The helper only supplies a name. Open and cleanup are
				// relative to the retained destination descriptor.
				let name = candidate.file_name().ok_or(io::ErrorKind::InvalidInput)?;
				let fd = fs::openat(directory, name, STAGE_FLAGS, PRIVATE_FILE)?;
				Ok((File::from(fd), name.to_os_string()))
			})
			.map_err(HostError::CreateStage)?;
		let ((file, name), _) = named.into_parts();
		Ok(Self {
			directory,
			name,
			file,
			committed: false,
		})
	}

	pub(super) fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
		self.file.write_all(bytes)
	}

	pub(super) fn sync(&self) -> io::Result<()> {
		self.file.sync_all()
	}

	pub(super) fn verify_name(&self) -> Result<(), HostError> {
		self.verified_stat().map(|_| ())
	}

	pub(super) fn verify_length(&self, expected: u64) -> Result<(), HostError> {
		let open = self.verified_stat()?;
		if u64::try_from(open.st_size).ok() != Some(expected) {
			return Err(HostError::StageChanged);
		}
		Ok(())
	}

	fn verified_stat(&self) -> Result<Stat, HostError> {
		let open = fs::fstat(&self.file).map_err(inspect)?;
		let current = stat_entry(self.directory, &self.name).map_err(inspect)?;
		if open.st_nlink != 1
			|| !current.is_some_and(|stat| {
				identity(&stat) == identity(&open)
					&& FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
			}) {
			return Err(HostError::StageChanged);
		}
		Ok(open)
	}

	pub(super) fn publish(&mut self, name: &HostName) -> Result<(), HostError> {
		fs::renameat_with(
			self.directory,
			&self.name,
			self.directory,
			name.as_str(),
			RenameFlags::NOREPLACE,
		)
		.map_err(|error| match error {
			Errno::EXIST => HostError::Conflict,
			Errno::NOSYS | Errno::NOTSUP | Errno::INVAL => {
				HostError::NoClobberUnavailable(error.into())
			}
			_ => HostError::Commit(error.into()),
		})?;
		self.committed = true;
		Ok(())
	}
}

impl Drop for Stage<'_> {
	fn drop(&mut self) {
		if !self.committed
			&& let Ok(open) = fs::fstat(&self.file)
			&& let Ok(Some(current)) = stat_entry(self.directory, &self.name)
			&& identity(&open) == identity(&current)
		{
			let _ = fs::unlinkat(self.directory, &self.name, AtFlags::empty());
		}
	}
}
