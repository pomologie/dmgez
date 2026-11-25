use std::{
	ffi::OsStr,
	fs::File,
	io::{self, Read, Write},
	os::unix::ffi::OsStrExt,
};

use rustix::{
	fd::OwnedFd,
	fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat, Timespec, Timestamps},
	io::Errno,
};

use super::{FILE_MODE, MAX_BLOCK_BYTES, owned_regular_file, private_file};

const KEY_BYTES: usize = 32;
const HEX_DIGITS_PER_BYTE: usize = 2;
const HEX_NIBBLE_BITS: usize = 4;
const LOW_NIBBLE: u8 = 0x0f;
const HEX: &[u8; 16] = b"0123456789abcdef";
const PREFIX: &[u8] = b"block-v1-";
const NAME_BYTES: usize = PREFIX.len() + KEY_BYTES * HEX_DIGITS_PER_BYTE;
pub(super) const PENDING: &str = ".dmgez-cache-pending-v1";
const MAX_CACHE_ENTRIES: usize = 4096;
const MAX_DIRECTORY_RECORDS: usize = MAX_CACHE_ENTRIES * 2;
const FILE_FLAGS: OFlags = OFlags::NOFOLLOW
	.union(OFlags::NONBLOCK)
	.union(OFlags::NOCTTY)
	.union(OFlags::CLOEXEC);

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Name([u8; NAME_BYTES]);

impl Name {
	pub(super) fn for_key(key: &[u8; KEY_BYTES]) -> Self {
		let mut name = [0; NAME_BYTES];
		name[..PREFIX.len()].copy_from_slice(PREFIX);
		let (digits, _) = name[PREFIX.len()..].as_chunks_mut::<HEX_DIGITS_PER_BYTE>();
		for (&byte, digits) in key.iter().zip(digits) {
			digits[0] = HEX[usize::from(byte >> HEX_NIBBLE_BITS)];
			digits[1] = HEX[usize::from(byte & LOW_NIBBLE)];
		}
		Self(name)
	}

	fn parse(bytes: &[u8]) -> Option<Self> {
		let name: [u8; NAME_BYTES] = bytes.try_into().ok()?;
		(name.starts_with(PREFIX) && name[PREFIX.len()..].iter().all(|byte| HEX.contains(byte)))
			.then_some(Self(name))
	}

	pub(super) fn as_os_str(&self) -> &OsStr {
		OsStr::from_bytes(&self.0)
	}
}

struct Entry {
	name: Name,
	stat: Stat,
	bytes: u64,
}

pub(super) fn load(
	directory: &OwnedFd,
	key: &[u8; KEY_BYTES],
	output: &mut [u8],
	max_bytes: u64,
) -> io::Result<usize> {
	let name = Name::for_key(key);
	let original = fs::statat(directory, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)?;
	if FileType::from_raw_mode(original.st_mode) != FileType::RegularFile {
		return Err(io::ErrorKind::InvalidData.into());
	}
	let source = fs::openat(
		directory,
		name.as_os_str(),
		FILE_FLAGS | OFlags::RDONLY,
		Mode::empty(),
	)?;
	let opened = fs::fstat(&source)?;
	if original.st_dev != opened.st_dev
		|| original.st_ino != opened.st_ino
		|| !private_file(&source, &opened)?
	{
		return Err(io::ErrorKind::PermissionDenied.into());
	}
	let length = usize::try_from(opened.st_size)
		.ok()
		.filter(|&len| {
			len > 0 && len <= output.len().min(MAX_BLOCK_BYTES) && len as u64 <= max_bytes
		})
		.ok_or(io::ErrorKind::InvalidData)?;
	let mut source = File::from(source);
	source.read_exact(&mut output[..length])?;
	let mut extra = [0];
	if source.read(&mut extra)? != 0 {
		return Err(io::ErrorKind::InvalidData.into());
	}
	let now = Timespec {
		tv_sec: 0,
		tv_nsec: fs::UTIME_NOW,
	};
	// Age updates use the opened object and never reopen its name.
	let _ = fs::futimens(
		&source,
		&Timestamps {
			last_access: now,
			last_modification: now,
		},
	);
	Ok(length)
}

pub(super) fn store(
	directory: &OwnedFd,
	key: &[u8; KEY_BYTES],
	bytes: &[u8],
	max_bytes: u64,
) -> io::Result<()> {
	let name = Name::for_key(key);
	let entries = collect_entries(directory)?;
	// A stage can predate its final mode adjustment after a crash.
	// Never retire a foreign file or a link to an inode outside the cache.
	remove_cache_file_if_present(directory, PENDING.as_ref())?;
	// Removing a previous value under the exclusive lock makes room before
	// staging, so the temporary payload also fits inside the byte cap.
	remove_cache_file_if_present(directory, name.as_os_str())?;
	make_room(directory, entries, name, bytes.len() as u64, max_bytes)?;

	let pending = fs::openat(
		directory,
		PENDING,
		FILE_FLAGS | OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
		FILE_MODE,
	)?;
	let mut pending = File::from(pending);
	let result = (|| {
		fs::fchmod(&pending, FILE_MODE)?;
		if !private_file(&pending, &fs::fstat(&pending)?)? {
			return Err(io::ErrorKind::PermissionDenied.into());
		}
		pending.write_all(bytes)?;
		fs::renameat(directory, PENDING, directory, name.as_os_str())?;
		Ok(())
	})();
	if result.is_err() {
		let _ = fs::unlinkat(directory, PENDING, AtFlags::empty());
	}
	result
}

fn remove_cache_file_if_present(directory: &OwnedFd, name: &OsStr) -> io::Result<()> {
	match fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) if owner_only_file(&stat) => Ok(fs::unlinkat(directory, name, AtFlags::empty())?),
		Ok(_) => Err(io::ErrorKind::InvalidData.into()),
		Err(Errno::NOENT) => Ok(()),
		Err(error) => Err(error.into()),
	}
}

fn owner_only_file(stat: &Stat) -> bool {
	owned_regular_file(stat)
		&& Mode::from_raw_mode(stat.st_mode)
			.difference(FILE_MODE)
			.is_empty()
}

fn collect_entries(directory: &OwnedFd) -> io::Result<Vec<Entry>> {
	let mut entries = Vec::new();
	for (count, item) in Dir::read_from(directory)?.enumerate() {
		if count == MAX_DIRECTORY_RECORDS {
			return Err(io::Error::other("cache directory exceeds the scan limit"));
		}
		let item = item?;
		let Some(name) = Name::parse(item.file_name().to_bytes()) else {
			continue;
		};
		let stat = fs::statat(directory, name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)?;
		if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
			if !owner_only_file(&stat) {
				return Err(io::ErrorKind::PermissionDenied.into());
			}
			let bytes = u64::try_from(stat.st_size).map_err(|_| io::ErrorKind::InvalidData)?;
			entries.try_reserve(1).map_err(io::Error::other)?;
			entries.push(Entry { name, stat, bytes });
		}
	}
	Ok(entries)
}

fn make_room(
	directory: &OwnedFd,
	mut entries: Vec<Entry>,
	excluded: Name,
	new_bytes: u64,
	max_bytes: u64,
) -> io::Result<()> {
	entries.retain(|entry| entry.name != excluded);
	let mut used = entries
		.iter()
		.map(|entry| u128::from(entry.bytes))
		.sum::<u128>()
		+ u128::from(new_bytes);
	let mut count = entries.len() + 1;
	if used <= u128::from(max_bytes) && count <= MAX_CACHE_ENTRIES {
		return Ok(());
	}
	entries
		.sort_unstable_by_key(|entry| (entry.stat.st_mtime, entry.stat.st_mtime_nsec, entry.name));
	for entry in entries {
		remove_cache_file_if_present(directory, entry.name.as_os_str())?;
		used -= u128::from(entry.bytes);
		count -= 1;
		if used <= u128::from(max_bytes) && count <= MAX_CACHE_ENTRIES {
			break;
		}
	}
	Ok(())
}
