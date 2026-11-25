//! Open and authorize an existing regular file for content replacement.

use std::{
	fs::File,
	io::{Read, Write},
	path::Path,
};

#[cfg(target_os = "macos")]
use std::os::fd::AsFd as _;

use rustix::{
	fs::{self, Mode, OFlags},
	io::Errno,
};

use super::{
	AccessMode, EntryIdentity, ReplaceError, StagedReader, inspect, matches_stamp,
	publish_replacement,
};

const EXISTING_FLAGS: OFlags = OFlags::RDWR
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);
const ACCESS_PERMISSIONS: Mode = Mode::RWXU.union(Mode::RWXG).union(Mode::RWXO);
const SPECIAL_PERMISSIONS: Mode = Mode::SUID.union(Mode::SGID).union(Mode::SVTX);
#[cfg(target_os = "macos")]
const HOST_PROVENANCE: &[u8] = b"com.apple.provenance";
#[cfg(target_os = "macos")]
const ATTRIBUTE_LIST_BYTES: usize = 4096;

/// Replaces a known regular file after validating its contents from a held handle.
///
/// The returned identity belongs to the published file. This preserves ordinary
/// access bits and requires the new file to have the same owner and group.
/// Multiple hard links, special permission bits, and extended metadata prevent
/// replacement, apart from host-generated macOS provenance. The caller must
/// use a trusted local directory whose publishers cooperate with the advisory
/// lock, as described by [publish_replace](super::publish_replace).
///
/// # Errors
/// Returns admission, authorization, staging, validation, or atomic publication
/// errors. A missing or replaced incumbent fails the expected-identity check.
pub fn publish_replace_existing<T, E>(
	destination: &Path,
	expected: EntryIdentity,
	authorize: impl FnOnce(&mut dyn Read) -> Result<(), E>,
	write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
	validate: impl FnOnce(StagedReader<'_>, &T) -> Result<(), E>,
) -> Result<(T, EntryIdentity), ReplaceError<E>> {
	publish_replacement(
		destination,
		|directory, name, target, original| {
			let original = original
				.filter(|_| target.existing == Some(expected))
				.ok_or(ReplaceError::DestinationChanged)?;
			let fd =
				fs::openat(directory, name, EXISTING_FLAGS, Mode::empty()).map_err(|error| {
					match error {
						Errno::NOENT | Errno::NOTDIR | Errno::LOOP => {
							ReplaceError::DestinationChanged
						}
						error => inspect(error),
					}
				})?;
			let held = fs::fstat(&fd).map_err(inspect)?;
			if !matches_stamp(original, &held) {
				return Err(ReplaceError::DestinationChanged);
			}
			let mode = Mode::from_raw_mode(held.st_mode);
			if held.st_nlink != 1 || mode.intersects(SPECIAL_PERMISSIONS) {
				return Err(ReplaceError::DestinationMetadata);
			}
			let mut file = File::from(fd);
			check_extended_metadata(&file)?;
			authorize(&mut file).map_err(ReplaceError::Authorize)?;
			Ok(Some(AccessMode {
				owner: held.st_uid,
				group: held.st_gid,
				mode: mode & ACCESS_PERMISSIONS,
			}))
		},
		write,
		validate,
	)
}

pub(super) fn check_extended_metadata<E>(file: &File) -> Result<(), ReplaceError<E>> {
	#[cfg(target_os = "linux")]
	if fs::flistxattr_query(file).map_err(inspect)? != 0 {
		return Err(ReplaceError::DestinationMetadata);
	}
	#[cfg(target_os = "macos")]
	{
		if !crate::private_directory::no_extended_acl(file.as_fd())
			.map_err(ReplaceError::Inspect)?
		{
			return Err(ReplaceError::DestinationMetadata);
		}
		let mut list = [0; ATTRIBUTE_LIST_BYTES];
		let count = fs::flistxattr(file, &mut list[..]).map_err(inspect)?;
		if list[..count]
			.split(|&byte| byte == 0)
			.filter(|name| !name.is_empty())
			.any(|name| name != HOST_PROVENANCE)
		{
			return Err(ReplaceError::DestinationMetadata);
		}
	}
	Ok(())
}
