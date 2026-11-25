use std::{convert::Infallible, error::Error as StdError, fmt, path::PathBuf};

use rustix::{
	fd::AsFd,
	fs::{self, AtFlags, Dir, FileType, Stat},
	io::Errno,
};

use crate::extract::HostName;

use super::super::readback::{CURRENT_DIRECTORY, PARENT_DIRECTORY};
use super::{Check, DestinationDirectory, ExistingBundleError, check_cancelled, os};

const MAX_PARENT_ENTRIES: usize = 16_384;
const MNT_LOCAL: u32 = 0x0000_1000; // Darwin sys/mount.h
const SF_DATALESS: u32 = 0x4000_0000; // Darwin sys/stat.h

/// Opaque observation of an existing bundle's root directory.
///
/// Equality includes the observed root metadata and change timestamps. It
/// does not attest to unchanged descendants. Use a verified bundle snapshot
/// to check the entire admitted tree.
#[derive(Clone, Copy)]
pub struct BundleStamp(pub(super) Stat);

/// Opaque filesystem object identity separated from its change metadata.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct BundleIdentity {
	device: i32,
	inode: u64,
	generation: u32,
	birth_seconds: i64,
	birth_nanoseconds: i64,
	kind: FileType,
}

impl BundleStamp {
	/// Returns the object identity independently of its mutation stamp.
	#[must_use]
	pub fn identity(&self) -> BundleIdentity {
		identity(&self.0)
	}

	/// Whether the observations refer to the same filesystem object identity.
	#[must_use]
	pub fn same_object(&self, other: &Self) -> bool {
		self.identity() == other.identity()
	}
}

impl PartialEq for BundleStamp {
	fn eq(&self, other: &Self) -> bool {
		unchanged(&self.0, &other.0)
	}
}

impl Eq for BundleStamp {}

impl fmt::Debug for BundleStamp {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("BundleStamp")
			.finish_non_exhaustive()
	}
}

impl fmt::Debug for BundleIdentity {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("BundleIdentity")
			.finish_non_exhaustive()
	}
}

/// Observes the exact named bundle beneath the held, authorized local parent.
///
/// `None` means the name was absent in a consistent observation. A symlink,
/// case alias, non-directory, or other inadmissible entry is a typed failure.
/// This checks the root binding and does not read application payloads.
///
/// # Errors
/// Returns a typed error for an inadmissible bundle, a changed namespace,
/// an exceeded scan bound, or a host inspection failure.
pub fn observe_existing_bundle(
	destination: &DestinationDirectory<'_>,
	name: &HostName,
) -> Result<Option<BundleStamp>, ExistingBundleError<Infallible>> {
	Ok(open_bundle(destination, name, &mut || false)?.map(|bundle| bundle.stamp))
}

pub(super) struct ParentBinding {
	path: PathBuf,
	stamp: Stat,
}

impl ParentBinding {
	pub(super) fn observe<E: StdError + 'static>(
		destination: &DestinationDirectory<'_>,
	) -> Check<Self, E> {
		let path = destination.attached_path()?;
		let stamp = os(fs::fstat(destination.descriptor()))?;
		destination.check_attachment()?;
		Ok(Self { path, stamp })
	}

	pub(super) fn recheck<E: StdError + 'static>(
		&self,
		destination: &DestinationDirectory<'_>,
	) -> Check<(), E> {
		let current = Self::observe(destination)?;
		if self.path != current.path || identity(&self.stamp) != identity(&current.stamp) {
			return Err(ExistingBundleError::Changed);
		}
		Ok(())
	}
}

pub(super) struct ScopedBundle<'root> {
	pub(super) stamp: BundleStamp,
	pub(super) directory: DestinationDirectory<'root>,
}

pub(super) fn open_bundle<'root, E: StdError + 'static>(
	destination: &DestinationDirectory<'root>,
	name: &HostName,
	cancelled: &mut dyn FnMut() -> bool,
) -> Check<Option<ScopedBundle<'root>>, E> {
	check_cancelled(cancelled)?;
	if os(fs::fstatfs(destination.descriptor()))?.f_flags & MNT_LOCAL == 0 {
		return Err(ExistingBundleError::NonLocal);
	}
	let Some(initial) = observe_bundle_name(destination, name.as_str(), cancelled)? else {
		return Ok(None);
	};
	if kind(&initial) != FileType::Directory {
		return Err(ExistingBundleError::Mismatch);
	}
	check_materialized(&initial)?;
	let directory = destination.open_child(name)?;
	check_stamp(&initial, &os(fs::fstat(directory.descriptor()))?)?;
	check_name(destination.descriptor(), name.as_str(), &initial)?;
	directory.check_attachment()?;
	Ok(Some(ScopedBundle {
		stamp: BundleStamp(initial),
		directory,
	}))
}

pub(super) fn observe_bundle_name<E: StdError + 'static>(
	parent: &DestinationDirectory<'_>,
	name: &str,
	cancelled: &mut dyn FnMut() -> bool,
) -> Check<Option<Stat>, E> {
	check_cancelled(cancelled)?;
	parent.check_attachment()?;
	let fd = parent.descriptor();
	let stamp = os(fs::fstat(fd))?;
	let initial = named(fd, name)?;
	if let Some(initial) = &initial {
		let mut count = 0;
		let mut exact = false;
		for item in os(Dir::read_from(fd))? {
			check_cancelled(cancelled)?;
			let item = os(item)?;
			let entry = item.file_name().to_bytes();
			if entry == CURRENT_DIRECTORY || entry == PARENT_DIRECTORY {
				continue;
			}
			if count == MAX_PARENT_ENTRIES {
				return Err(ExistingBundleError::ParentLimit);
			}
			if entry == name.as_bytes() {
				exact = true;
				break;
			}
			count += 1;
		}
		check_name(fd, name, initial)?;
		check_stamp(&stamp, &os(fs::fstat(fd))?)?;
		if !exact {
			return Err(ExistingBundleError::Mismatch);
		}
	} else if named(fd, name)?.is_some() {
		return Err(ExistingBundleError::Changed);
	}
	check_stamp(&stamp, &os(fs::fstat(fd))?)?;
	parent.check_attachment()?;
	Ok(initial)
}

pub(super) fn named<E: StdError + 'static>(fd: impl AsFd, name: &str) -> Check<Option<Stat>, E> {
	match fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) => Ok(Some(stat)),
		Err(Errno::NOENT) => Ok(None),
		Err(error) => Err(ExistingBundleError::Io(error.into())),
	}
}

pub(super) fn check_name<E: StdError + 'static>(
	fd: impl AsFd,
	name: &str,
	before: &Stat,
) -> Check<(), E> {
	let current = named(fd, name)?.ok_or(ExistingBundleError::Changed)?;
	check_stamp(before, &current)
}

pub(super) fn check_stamp<E: StdError + 'static>(before: &Stat, current: &Stat) -> Check<(), E> {
	if !unchanged(before, current) {
		return Err(ExistingBundleError::Changed);
	}
	Ok(())
}

pub(super) fn check_materialized<E: StdError + 'static>(stat: &Stat) -> Check<(), E> {
	if stat.st_flags & SF_DATALESS != 0 {
		return Err(ExistingBundleError::Dataless);
	}
	Ok(())
}

fn identity(stat: &Stat) -> BundleIdentity {
	BundleIdentity {
		device: stat.st_dev,
		inode: stat.st_ino,
		generation: stat.st_gen,
		birth_seconds: stat.st_birthtime,
		birth_nanoseconds: stat.st_birthtime_nsec,
		kind: kind(stat),
	}
}

fn unchanged(before: &Stat, current: &Stat) -> bool {
	identity(before) == identity(current)
		&& before.st_ctime == current.st_ctime
		&& before.st_ctime_nsec == current.st_ctime_nsec
		&& before.st_mtime == current.st_mtime
		&& before.st_mtime_nsec == current.st_mtime_nsec
		&& before.st_mode == current.st_mode
		&& before.st_uid == current.st_uid
		&& before.st_gid == current.st_gid
		&& before.st_size == current.st_size
		&& before.st_nlink == current.st_nlink
		&& before.st_flags == current.st_flags
}

pub(super) fn kind(stat: &Stat) -> FileType {
	FileType::from_raw_mode(stat.st_mode)
}
