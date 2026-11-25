use std::{
	fs::File,
	io::{Read, Write},
};

use dmgez_core::private_directory::no_extended_acl;
use rustix::{
	fd::{AsFd, OwnedFd},
	fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, Stat},
	io::Errno,
	process::geteuid,
};

use super::{Error, SessionRecord, directory::same_entry};

pub(super) const RECORD: &str = "session.json";
pub(super) const PENDING_RECORD: &str = ".session.pending";
pub(super) const MAX_RECORD_BYTES: usize = 32 * 1024;
const EXTRA_PROBE_BYTES: usize = 1;
const SINGLE_LINK: u64 = 1;
const PRIVATE_RECORD_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const READ_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);
const WRITE_FLAGS: OFlags = OFlags::WRONLY
	.union(OFlags::CREATE)
	.union(OFlags::EXCL)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);

pub(super) fn read(directory: &OwnedFd) -> Result<(SessionRecord, Stat), Error> {
	let file = read_file(directory, RECORD)?;
	let expected = require_private(&file)?;
	let length = usize::try_from(expected.st_size).map_err(|_| Error::Limit)?;
	if length == 0 || length > MAX_RECORD_BYTES {
		return Err(Error::Limit);
	}
	let mut file = File::from(file);
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(length)?;
	bytes.resize(length, 0);
	file.read_exact(&mut bytes)?;
	if file.read(&mut [0; EXTRA_PROBE_BYTES])? != 0 {
		return Err(Error::RecordChanged);
	}
	let after = require_private(&file)?;
	let named = fs::statat(directory, RECORD, AtFlags::SYMLINK_NOFOLLOW)?;
	if !same_stamp(&expected, &after) || !same_stamp(&expected, &named) {
		return Err(Error::RecordChanged);
	}
	Ok((serde_json::from_slice(&bytes)?, after))
}

pub(super) fn write_initial(directory: &OwnedFd, record: &SessionRecord) -> Result<(), Error> {
	publish(directory, record, Target::Absent)
}

pub(super) fn replace(
	directory: &OwnedFd,
	record: &SessionRecord,
	stamp: &Stat,
) -> Result<(), Error> {
	publish(directory, record, Target::Existing(stamp))
}

enum Target<'stamp> {
	Absent,
	Existing(&'stamp Stat),
}

fn publish(directory: &OwnedFd, record: &SessionRecord, target: Target<'_>) -> Result<(), Error> {
	let body = serde_json::to_vec(record)?;
	if body.len() > MAX_RECORD_BYTES {
		return Err(Error::Limit);
	}
	// Only the registry lock owner may retire an interrupted private stage.
	// Never unlink malformed, followed, unowned, or multiply linked names.
	if let Some(interrupted) = pending_file(directory)? {
		let held = require_private(&interrupted)?;
		let named = fs::statat(directory, PENDING_RECORD, AtFlags::SYMLINK_NOFOLLOW)?;
		if !same_stamp(&held, &named) {
			return Err(Error::RecordChanged);
		}
		fs::unlinkat(directory, PENDING_RECORD, AtFlags::empty())?;
	}
	let descriptor = fs::openat(directory, PENDING_RECORD, WRITE_FLAGS, PRIVATE_RECORD_MODE)?;
	let mut file = File::from(descriptor);
	require_private(&file)?;
	file.write_all(&body)?;
	file.sync_all()?;
	let stage = require_private(&file)?;
	let staged_name = fs::statat(directory, PENDING_RECORD, AtFlags::SYMLINK_NOFOLLOW)?;
	if !same_stamp(&stage, &staged_name) {
		return Err(Error::RecordChanged);
	}
	match target {
		Target::Absent => fs::renameat_with(
			directory,
			PENDING_RECORD,
			directory,
			RECORD,
			RenameFlags::NOREPLACE,
		)?,
		Target::Existing(previous) => {
			let named = fs::statat(directory, RECORD, AtFlags::SYMLINK_NOFOLLOW)?;
			if !same_stamp(previous, &named) {
				return Err(Error::RecordChanged);
			}
			fs::renameat(directory, PENDING_RECORD, directory, RECORD)?;
		}
	}
	fs::fsync(directory)?;
	Ok(())
}

fn pending_file(directory: &OwnedFd) -> Result<Option<OwnedFd>, Error> {
	match read_file(directory, PENDING_RECORD) {
		Ok(file) => Ok(Some(file)),
		Err(Error::Missing) => Ok(None),
		Err(error) => Err(error),
	}
}

fn read_file(directory: &OwnedFd, name: &str) -> Result<OwnedFd, Error> {
	// Inspect before open to avoid opening FIFOs or device nodes in a tampered
	// store and recheck the held object before consuming bytes.
	let named = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
		if error == Errno::NOENT {
			Error::Missing
		} else {
			error.into()
		}
	})?;
	if !private_metadata(&named) {
		return Err(Error::Privacy);
	}
	let file = fs::openat(directory, name, READ_FLAGS, Mode::empty())?;
	if !same_stamp(&named, &fs::fstat(&file)?) {
		return Err(Error::RecordChanged);
	}
	Ok(file)
}

fn require_private(file: impl AsFd) -> Result<Stat, Error> {
	let stat = fs::fstat(&file)?;
	if !private_metadata(&stat) || !no_extended_acl(file.as_fd())? {
		return Err(Error::Privacy);
	}
	Ok(stat)
}

fn private_metadata(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
		&& Mode::from_raw_mode(stat.st_mode) == PRIVATE_RECORD_MODE
		&& stat.st_uid == geteuid().as_raw()
		&& u64::from(stat.st_nlink) == SINGLE_LINK
}

fn same_stamp(first: &Stat, second: &Stat) -> bool {
	same_entry(first, second)
		&& first.st_mode == second.st_mode
		&& first.st_uid == second.st_uid
		&& first.st_nlink == second.st_nlink
		&& first.st_size == second.st_size
		&& first.st_mtime == second.st_mtime
		&& first.st_mtime_nsec == second.st_mtime_nsec
		&& first.st_ctime == second.st_ctime
		&& first.st_ctime_nsec == second.st_ctime_nsec
}
