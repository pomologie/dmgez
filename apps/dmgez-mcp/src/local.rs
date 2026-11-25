use std::{
	fs::File,
	io,
	os::fd::OwnedFd,
	path::{Component, Path},
};

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, Stat};

pub(super) const MAX_FILE_NAME_BYTES: usize = 255;
const HOST_ROOT: &str = "/";
const FORBIDDEN_FILE_NAME: [char; 3] = ['/', '\\', ':'];
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);

#[derive(Debug)]
pub(super) struct LocalRoot(pub(super) OwnedFd);

pub(super) enum FileAdmission {
	Unavailable,
	Changed,
}

pub(super) struct FileBinding<'a> {
	root: &'a LocalRoot,
	name: &'a Path,
	stamp: Stat,
}

impl LocalRoot {
	pub(super) fn open(root: &Path) -> io::Result<Self> {
		let absolute = std::path::absolute(root)?;
		let mut directory = fs::open(HOST_ROOT, DIRECTORY_FLAGS, Mode::empty())?;
		for component in absolute.components() {
			match component {
				Component::RootDir => (),
				Component::Normal(name) => {
					directory = fs::openat(&directory, name, DIRECTORY_FLAGS, Mode::empty())?;
				}
				_ => return Err(io::ErrorKind::InvalidInput.into()),
			}
		}
		Ok(Self(directory))
	}

	pub(super) fn open_file<'a>(
		&'a self,
		name: &'a Path,
	) -> Result<(File, FileBinding<'a>), FileAdmission> {
		let named = fs::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|_| FileAdmission::Unavailable)?;
		if !is_direct_file(&named) {
			return Err(FileAdmission::Unavailable);
		}
		let file = fs::openat(&self.0, name, FILE_FLAGS, Mode::empty())
			.map_err(|_| FileAdmission::Unavailable)?;
		let stamp = fs::fstat(&file).map_err(|_| FileAdmission::Unavailable)?;
		if !same_observation(&named, &stamp) {
			return Err(FileAdmission::Changed);
		}
		Ok((
			File::from(file),
			FileBinding {
				root: self,
				name,
				stamp,
			},
		))
	}
}

impl FileBinding<'_> {
	pub(super) fn verify(&self) -> Result<(), FileAdmission> {
		let observed =
			fs::statat(&self.root.0, self.name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
				if error == rustix::io::Errno::NOENT {
					FileAdmission::Changed
				} else {
					FileAdmission::Unavailable
				}
			})?;
		if same_observation(&self.stamp, &observed) {
			Ok(())
		} else {
			Err(FileAdmission::Changed)
		}
	}
}

pub(super) fn direct_name<'a>(file_name: &'a str, extension: &str) -> Option<&'a Path> {
	let path = Path::new(file_name);
	if file_name.len() > MAX_FILE_NAME_BYTES
		|| file_name.contains(FORBIDDEN_FILE_NAME)
		|| file_name.chars().any(char::is_control)
		|| path.extension() != Some(extension.as_ref())
	{
		return None;
	}
	Some(path)
}

pub(super) fn is_direct_file(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1
}

pub(super) fn same_observation(first: &Stat, second: &Stat) -> bool {
	first.st_dev == second.st_dev
		&& first.st_ino == second.st_ino
		&& first.st_mode == second.st_mode
		&& first.st_nlink == second.st_nlink
		&& first.st_size == second.st_size
		&& first.st_mtime == second.st_mtime
		&& first.st_mtime_nsec == second.st_mtime_nsec
		&& first.st_ctime == second.st_ctime
		&& first.st_ctime_nsec == second.st_ctime_nsec
}
