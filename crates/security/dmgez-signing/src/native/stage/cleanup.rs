use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use rustix::{
	fd::OwnedFd,
	fs::{self, AtFlags, Dir, FileType, Mode, Stat},
	io::{self, Errno},
};

use super::{
	CURRENT_DIRECTORY_BYTES, DIRECTORY_FLAGS, PARENT_DIRECTORY, PRIVATE_DIRECTORY_MODE,
	same_identity, stat_entry,
};

const MAX_DIRECTORY_DEPTH: usize = 128;
const MAX_CLEANUP_ENTRIES: usize = 1_000_000;

pub(super) fn remove_app(parent: &OwnedFd, name: &OsStr, expected_root: Option<&Stat>) {
	let Ok(tree) = stat_entry(parent, name) else {
		return;
	};
	if expected_root.is_some_and(|root| !same_identity(root, &tree)) {
		return;
	}
	let Ok(stage) = fs::fstat(parent) else {
		return;
	};
	if stage.st_dev != tree.st_dev {
		return;
	}
	let mut remaining = MAX_CLEANUP_ENTRIES;
	let _ = remove_entry(parent, name, &tree, 0, &mut remaining);
}

fn remove_entry(
	parent: &OwnedFd,
	name: &OsStr,
	entry: &Stat,
	depth: usize,
	remaining: &mut usize,
) -> io::Result<()> {
	*remaining = remaining.checked_sub(1).ok_or(Errno::NOSPC)?;
	if FileType::from_raw_mode(entry.st_mode) != FileType::Directory {
		return fs::unlinkat(parent, name, AtFlags::empty());
	}
	if depth == MAX_DIRECTORY_DEPTH {
		return Err(Errno::LOOP);
	}

	let directory = fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())?;
	if !same_identity(entry, &fs::fstat(&directory)?) {
		return Err(Errno::BUSY);
	}
	// ditto can copy read-only modes. Give owner write access through the
	// already verified private directory descriptor before removing entries.
	let mode = Mode::from_raw_mode(entry.st_mode);
	if !mode.contains(PRIVATE_DIRECTORY_MODE) {
		fs::fchmod(&directory, mode | PRIVATE_DIRECTORY_MODE)?;
	}
	for child in Dir::read_from(&directory)? {
		let child = child?;
		let component = child.file_name().to_bytes();
		if matches!(component, CURRENT_DIRECTORY_BYTES | PARENT_DIRECTORY) {
			continue;
		}
		let component = OsStr::from_bytes(component);
		let current = stat_entry(&directory, component)?;
		if current.st_dev != entry.st_dev {
			return Err(Errno::XDEV);
		}
		remove_entry(&directory, component, &current, depth + 1, remaining)?;
	}
	if !same_identity(entry, &stat_entry(parent, name)?) {
		return Err(Errno::BUSY);
	}
	fs::unlinkat(parent, name, AtFlags::REMOVEDIR)
}
