use std::{
	collections::{BTreeMap, BTreeSet},
	ffi::{CStr, OsStr},
	fs::{self, File},
	os::unix::ffi::OsStrExt,
	path::{Path, PathBuf},
};

use rustix::fs::{self as unix, AtFlags, Dir, FileType, Mode, OFlags, Stat};

use super::{CopyError, at, contents};

const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 100_000;
const MAX_PATH_BYTES: usize = 64 << 20;
const DOT: &[u8] = b".";
const DOT_DOT: &[u8] = b"..";
const READ_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);
const DIRECTORY_FLAGS: OFlags = READ_FLAGS.union(OFlags::DIRECTORY);

type Identity = (i32, u64);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Inspection {
	Full,
	Stamps,
}

pub(super) struct Inventory {
	entries: BTreeMap<PathBuf, Entry>,
}

struct Entry {
	stat: Stat,
	link: Option<PathBuf>,
	contents: Option<contents::Contents>,
}

impl Inventory {
	pub(super) fn read(root: &Path, inspection: Inspection) -> Result<Self, CopyError> {
		let dir = File::from(
			unix::open(root, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| at(root, error))?,
		);
		let stat = unix::fstat(&dir).map_err(|error| at(root, error))?;
		let mut walk = Walk {
			root,
			device: stat.st_dev,
			inspection,
			entries: BTreeMap::new(),
			directories: BTreeSet::new(),
			path_bytes: MAX_PATH_BYTES,
			budget: contents::Budget::new(),
		};
		walk.directory(&dir, Path::new(""), stat, 0)?;
		let observed = unix::statat(unix::CWD, root, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|error| at(root, error))?;
		if !same_stamp(&stat, &observed) {
			return Err(CopyError::SourceChanged(root.to_path_buf()));
		}
		for (relative, entry) in &walk.entries {
			if let Some(target) = &entry.link {
				validate_link(root, relative, target)?;
			}
		}
		Ok(Self {
			entries: walk.entries,
		})
	}

	pub(super) fn compare_source(&self, current: &Self) -> Result<(), CopyError> {
		for (path, expected) in &self.entries {
			let unchanged = current.entries.get(path).is_some_and(|actual| {
				same_stamp(&expected.stat, &actual.stat) && expected.link == actual.link
			});
			if !unchanged {
				return Err(CopyError::SourceChanged(path.clone()));
			}
		}
		if let Some(path) = self.additional_path(current) {
			return Err(CopyError::SourceChanged(path.clone()));
		}
		Ok(())
	}

	pub(super) fn compare_copy(&self, staged: &Self) -> Result<(), CopyError> {
		let source_ids: BTreeSet<_> = self
			.entries
			.values()
			.map(|entry| identity(&entry.stat))
			.collect();
		let mut old_to_new = BTreeMap::new();
		let mut new_to_old = BTreeMap::new();
		for (path, expected) in &self.entries {
			let Some(actual) = staged.entries.get(path) else {
				return Err(CopyError::CopyDiffers(path.clone()));
			};
			if expected.stat.st_mode != actual.stat.st_mode
				|| expected.link != actual.link
				|| expected.contents != actual.contents
			{
				return Err(CopyError::CopyDiffers(path.clone()));
			}
			let old = identity(&expected.stat);
			let new = identity(&actual.stat);
			if source_ids.contains(&new)
				|| old_to_new
					.insert(old, new)
					.is_some_and(|prior| prior != new)
				|| new_to_old
					.insert(new, old)
					.is_some_and(|prior| prior != old)
			{
				return Err(CopyError::CopyDiffers(path.clone()));
			}
		}
		if let Some(path) = self.additional_path(staged) {
			return Err(CopyError::CopyDiffers(path.clone()));
		}
		Ok(())
	}

	fn additional_path<'a>(&self, other: &'a Self) -> Option<&'a PathBuf> {
		(other.entries.len() != self.entries.len())
			.then(|| {
				other
					.entries
					.keys()
					.find(|path| !self.entries.contains_key(*path))
			})
			.flatten()
	}
}

struct Walk<'a> {
	root: &'a Path,
	device: i32,
	inspection: Inspection,
	entries: BTreeMap<PathBuf, Entry>,
	directories: BTreeSet<Identity>,
	path_bytes: usize,
	budget: contents::Budget,
}

impl Walk<'_> {
	fn directory(
		&mut self,
		dir: &File,
		relative: &Path,
		stat: Stat,
		depth: usize,
	) -> Result<(), CopyError> {
		if depth > MAX_DEPTH {
			return Err(CopyError::Limit("nesting depth"));
		}
		let path = self.root.join(relative);
		if !self.directories.insert(identity(&stat)) {
			return Err(CopyError::Unsupported(path));
		}
		let contents = self.content(dir, &path, &stat, false)?;
		self.insert(
			relative,
			Entry {
				stat,
				link: None,
				contents,
			},
		)?;
		let entries = Dir::read_from(dir).map_err(|error| at(&path, error))?;
		for entry in entries {
			let entry = entry.map_err(|error| at(&path, error))?;
			let name = entry.file_name();
			if !matches!(name.to_bytes(), DOT | DOT_DOT) {
				let child = relative.join(OsStr::from_bytes(name.to_bytes()));
				self.child(dir, name, &child, depth + 1)?;
			}
		}
		validate_handle(dir, &path, &stat)
	}

	fn child(
		&mut self,
		parent: &File,
		name: &CStr,
		relative: &Path,
		depth: usize,
	) -> Result<(), CopyError> {
		let path = self.root.join(relative);
		let stat = unix::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|error| at(&path, error))?;
		if stat.st_dev != self.device {
			return Err(CopyError::Unsupported(path));
		}
		match FileType::from_raw_mode(stat.st_mode) {
			FileType::Directory | FileType::RegularFile => {
				let directory = FileType::from_raw_mode(stat.st_mode) == FileType::Directory;
				let flags = if directory {
					DIRECTORY_FLAGS
				} else {
					READ_FLAGS
				};
				let file = File::from(
					unix::openat(parent, name, flags, Mode::empty())
						.map_err(|error| at(&path, error))?,
				);
				validate_handle(&file, &path, &stat)?;
				if directory {
					self.directory(&file, relative, stat, depth)?;
				} else {
					let contents = self.content(&file, &path, &stat, true)?;
					validate_handle(&file, &path, &stat)?;
					self.insert(
						relative,
						Entry {
							stat,
							link: None,
							contents,
						},
					)?;
				}
			}
			FileType::Symlink => {
				let target =
					unix::readlinkat(parent, name, Vec::new()).map_err(|error| at(&path, error))?;
				let target = PathBuf::from(OsStr::from_bytes(target.to_bytes()));
				self.insert(
					relative,
					Entry {
						stat,
						link: Some(target),
						contents: None,
					},
				)?;
			}
			_ => return Err(CopyError::Unsupported(path)),
		}
		let observed = unix::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|error| at(&path, error))?;
		if !same_stamp(&stat, &observed) {
			return Err(CopyError::SourceChanged(path));
		}
		Ok(())
	}

	fn content(
		&mut self,
		file: &File,
		path: &Path,
		stat: &Stat,
		regular: bool,
	) -> Result<Option<contents::Contents>, CopyError> {
		if self.inspection == Inspection::Stamps {
			return Ok(None);
		}
		let size = regular.then_some(stat.st_size);
		contents::Contents::read(file, path, size, &mut self.budget).map(Some)
	}

	fn insert(&mut self, relative: &Path, entry: Entry) -> Result<(), CopyError> {
		if self.entries.len() >= MAX_ENTRIES {
			return Err(CopyError::Limit("entry count"));
		}
		self.path_bytes = self
			.path_bytes
			.checked_sub(relative.as_os_str().as_bytes().len())
			.ok_or(CopyError::Limit("path bytes"))?;
		if self.entries.insert(relative.to_path_buf(), entry).is_some() {
			return Err(CopyError::SourceChanged(self.root.join(relative)));
		}
		Ok(())
	}
}

fn validate_link(root: &Path, relative: &Path, target: &Path) -> Result<(), CopyError> {
	let path = root.join(relative);
	// An absolute link can address the original bundle even if it resolves
	// within that bundle before copying. Never admit one to the stage.
	if target.is_absolute() {
		return Err(CopyError::Link(path));
	}
	match fs::canonicalize(&path) {
		Ok(resolved) if resolved.starts_with(root) => Ok(()),
		_ => Err(CopyError::Link(path)),
	}
}

fn validate_handle(file: &File, path: &Path, expected: &Stat) -> Result<(), CopyError> {
	if !same_stamp(
		expected,
		&unix::fstat(file).map_err(|error| at(path, error))?,
	) {
		return Err(CopyError::SourceChanged(path.to_path_buf()));
	}
	Ok(())
}

fn identity(stat: &Stat) -> Identity {
	(stat.st_dev, stat.st_ino)
}

fn same_stamp(left: &Stat, right: &Stat) -> bool {
	identity(left) == identity(right)
		&& left.st_mode == right.st_mode
		&& left.st_size == right.st_size
		&& left.st_nlink == right.st_nlink
		&& left.st_uid == right.st_uid
		&& left.st_gid == right.st_gid
		&& left.st_flags == right.st_flags
		&& (left.st_mtime, left.st_mtime_nsec) == (right.st_mtime, right.st_mtime_nsec)
		&& (left.st_ctime, left.st_ctime_nsec) == (right.st_ctime, right.st_ctime_nsec)
}
