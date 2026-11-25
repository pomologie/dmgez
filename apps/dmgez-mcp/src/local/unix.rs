use std::{
	ffi::OsStr,
	fs::File,
	io,
	os::unix::ffi::OsStrExt as _,
	path::{Component, Path},
};

use rustix::fs::{self, AtFlags, Dir, DirEntry, FileType, Mode, OFlags, Stat};

use super::ScanError;

const HOST_ROOT: &str = "/";
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);

#[derive(Debug)]
pub(crate) struct LocalRoot(File);

pub(super) struct Stamp(Stat);

pub(super) struct Directory(Dir);

pub(super) struct Entry(DirEntry);

impl LocalRoot {
	pub(crate) fn open(root: &Path) -> io::Result<Self> {
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
		Ok(Self(directory.into()))
	}

	pub(super) fn named_stamp(&self, name: &Path) -> io::Result<Stamp> {
		Ok(Stamp(fs::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW)?))
	}

	pub(super) fn open_data(&self, name: &Path) -> io::Result<File> {
		Ok(fs::openat(&self.0, name, FILE_FLAGS, Mode::empty())?.into())
	}

	pub(super) fn directory_stamp(&self) -> io::Result<Stamp> {
		Stamp::capture(&self.0)
	}

	pub(super) fn directory(&self) -> io::Result<Directory> {
		Ok(Directory(Dir::read_from(&self.0)?))
	}

	pub(super) fn is_direct_entry(&self, entry: &Entry) -> Result<bool, ScanError> {
		let stamp = self.named_stamp(Path::new(entry.name())).map_err(|error| {
			if error.kind() == io::ErrorKind::NotFound {
				ScanError::Changed
			} else {
				ScanError::Unavailable
			}
		})?;
		if !stamp.is_direct_file() {
			return Ok(false);
		}
		if stamp.0.st_ino != entry.0.ino() {
			return Err(ScanError::Changed);
		}
		Ok(true)
	}
}

impl Stamp {
	pub(super) fn capture(file: &File) -> io::Result<Self> {
		Ok(Self(fs::fstat(file)?))
	}

	pub(super) fn is_direct_file(&self) -> bool {
		FileType::from_raw_mode(self.0.st_mode) == FileType::RegularFile && self.0.st_nlink == 1
	}
}

impl PartialEq for Stamp {
	fn eq(&self, other: &Self) -> bool {
		let (first, second) = (&self.0, &other.0);
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
}

impl Directory {
	pub(super) fn next_entry(&mut self) -> io::Result<Option<Entry>> {
		Ok(self.0.next().transpose()?.map(Entry))
	}
}

impl Entry {
	pub(super) fn name(&self) -> &OsStr {
		OsStr::from_bytes(self.0.file_name().to_bytes())
	}
}
