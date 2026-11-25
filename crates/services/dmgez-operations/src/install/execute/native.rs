use std::{
	error::Error as StdError,
	fs::File,
	io::{Read, Write},
	os::unix::ffi::OsStrExt,
};

use dmgez_vfs::volume::{Fork, ForkKind, NodeHandle, VolumeBackend};
use rustix::{
	fd::AsFd,
	fs::{self, AtFlags, FileType, Mode, OFlags, XattrFlags},
};

use crate::{
	extract::{HostName, host::DestinationDirectory},
	install::{
		CopyContent, CopyEntry,
		copy_plan::{FINDER_ATTRIBUTE, RESOURCE_ATTRIBUTE},
	},
};

use super::{HostMetadataError, InstallFailure, Run};

const CHUNK_BYTES: usize = 64 << 10;
const SYMLINK_MODE: u16 = 0o777;
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);
const SAFE_OPEN: OFlags = OFlags::CLOEXEC
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK);
const CREATE_FILE: OFlags = SAFE_OPEN
	.union(OFlags::CREATE)
	.union(OFlags::EXCL)
	.union(OFlags::WRONLY);
const READ_FILE: OFlags = SAFE_OPEN.union(OFlags::RDONLY);
const USER_ATTRIBUTE: &str = "user.";
const PROTECTED_PREFIXES: [&str; 5] = [
	"system.",
	"security.",
	"trusted.",
	"com.apple.system.",
	"com.apple.security.",
];
const PROTECTED_ATTRIBUTES: [&str; 4] = [
	"com.apple.macl",
	"com.apple.provenance",
	"com.apple.quarantine",
	"com.apple.rootless",
];

pub(super) fn host_supports(entry: &CopyEntry) -> bool {
	if cfg!(target_os = "linux") {
		return (!matches!(entry.content, CopyContent::Link(_)) || entry.mode == SYMLINK_MODE)
			&& entry.finder_info.is_none()
			&& !matches!(
				entry.content,
				CopyContent::File {
					resource_bytes: Some(_),
					..
				}
			) && entry
			.attributes
			.iter()
			.all(|item| item.host_name.starts_with(USER_ATTRIBUTE));
	}
	entry.attributes.iter().all(|item| {
		let name = item.host_name.as_str();
		!PROTECTED_ATTRIBUTES.contains(&name)
			&& !PROTECTED_PREFIXES
				.iter()
				.any(|prefix| name.starts_with(prefix))
	})
}

impl<B: VolumeBackend> Run<'_, '_, B> {
	pub(super) fn write_file(
		&mut self,
		source: &NodeHandle<B::Id>,
		entry: &CopyEntry,
		parent: &DestinationDirectory<'_>,
		name: &HostName,
	) -> Result<(), InstallFailure<B::Error>> {
		let CopyContent::File {
			data_bytes,
			resource_bytes,
			resource_alias,
		} = &entry.content
		else {
			return Err(InstallFailure::SourceChanged);
		};
		parent.check_attachment()?;
		let directory = parent.descriptor();
		let name = name.as_str();
		let mut writer = File::from(os(fs::openat(directory, name, CREATE_FILE, PRIVATE_FILE))?);
		self.view
			.with_fork(source, ForkKind::Data, |fork| {
				transfer(fork, *data_bytes, self.cancelled, |bytes| {
					writer.write_all(bytes).map_err(Into::into)
				})
			})?
			.ok_or(InstallFailure::SourceChanged)??;
		writer.sync_all()?;
		self.check_ready()?;
		let mut reader = File::from(os(fs::openat(directory, name, READ_FILE, Mode::empty()))?);
		verify_file(&writer, &reader, *data_bytes)?;
		let mut buffer = [0; CHUNK_BYTES];
		self.view
			.with_fork(source, ForkKind::Data, |fork| {
				transfer(fork, *data_bytes, self.cancelled, |bytes| {
					let actual = &mut buffer[..bytes.len()];
					reader.read_exact(actual)?;
					if actual != bytes {
						return Err(InstallFailure::ReadBack);
					}
					Ok(())
				})
			})?
			.ok_or(InstallFailure::SourceChanged)??;
		if reader.read(&mut [0])? != 0 {
			return Err(InstallFailure::ReadBack);
		}
		if let Some(bytes) = *resource_bytes {
			let value = self
				.view
				.with_fork(source, ForkKind::Resource, |fork| {
					read_value(fork, bytes, self.cancelled)
				})?
				.ok_or(InstallFailure::SourceChanged)??;
			if let Some(alias) = resource_alias {
				let attribute = self
					.view
					.with_xattr(source, alias.as_lookup(), |fork| {
						read_value(fork, bytes, self.cancelled)
					})?
					.ok_or(InstallFailure::SourceChanged)??;
				if attribute != value {
					return Err(InstallFailure::SourceChanged);
				}
			}
			set_attribute(&reader, RESOURCE_ATTRIBUTE, &value)?;
		}
		self.write_attributes(source, entry, &reader)?;
		apply_mode(&reader, entry.mode)?;
		writer.sync_all()?;
		parent.check_attachment()?;
		let opened = os(fs::fstat(&reader))?;
		let named = os(fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW))?;
		if !same_file(&opened, &named, *data_bytes) {
			return Err(InstallFailure::ReadBack);
		}
		Ok(())
	}

	pub(super) fn write_attributes(
		&mut self,
		source: &NodeHandle<B::Id>,
		entry: &CopyEntry,
		fd: impl AsFd,
	) -> Result<(), InstallFailure<B::Error>> {
		if let Some(value) = &entry.finder_info {
			set_attribute(fd.as_fd(), FINDER_ATTRIBUTE, value)?;
		}
		for item in &entry.attributes {
			self.check_ready()?;
			let value = self
				.view
				.with_xattr(source, item.source_name.as_lookup(), |fork| {
					read_value(fork, item.bytes, self.cancelled)
				})?
				.ok_or(InstallFailure::SourceChanged)??;
			set_attribute(fd.as_fd(), &item.host_name, &value)?;
		}
		Ok(())
	}
}

fn transfer<E: StdError + 'static>(
	fork: &mut Fork<'_, E>,
	expected: u64,
	cancelled: &mut dyn FnMut() -> bool,
	mut consume: impl FnMut(&[u8]) -> Result<(), InstallFailure<E>>,
) -> Result<(), InstallFailure<E>> {
	if fork.len() != expected {
		return Err(InstallFailure::SourceChanged);
	}
	let mut buffer = [0; CHUNK_BYTES];
	let mut offset = 0;
	while offset < expected {
		if cancelled() {
			return Err(InstallFailure::Cancelled);
		}
		let count = (expected - offset).min(CHUNK_BYTES as u64) as usize;
		let bytes = &mut buffer[..count];
		fork.read_exact_at(offset, bytes)?;
		consume(bytes)?;
		offset += count as u64;
	}
	Ok(())
}

fn read_value<E: StdError + 'static>(
	fork: &mut Fork<'_, E>,
	bytes: u64,
	cancelled: &mut dyn FnMut() -> bool,
) -> Result<Vec<u8>, InstallFailure<E>> {
	let mut value = Vec::new();
	value.try_reserve_exact(usize::try_from(bytes).map_err(|_| HostMetadataError)?)?;
	transfer(fork, bytes, cancelled, |chunk| {
		value.extend_from_slice(chunk);
		Ok(())
	})?;
	Ok(value)
}

fn set_attribute<E: StdError + 'static>(
	fd: impl AsFd,
	name: &str,
	value: &[u8],
) -> Result<(), InstallFailure<E>> {
	os(fs::fsetxattr(fd.as_fd(), name, value, XattrFlags::CREATE))?;
	if os(fs::fgetxattr_query(fd.as_fd(), name))? != value.len() {
		return Err(InstallFailure::ReadBack);
	}
	if !value.is_empty() {
		let mut actual = Vec::new();
		actual.try_reserve_exact(value.len())?;
		actual.resize(value.len(), 0);
		let count = os(fs::fgetxattr(fd.as_fd(), name, actual.as_mut_slice()))?;
		if count != value.len() || actual != value {
			return Err(InstallFailure::ReadBack);
		}
	}
	Ok(())
}

fn verify_file<E: StdError + 'static>(
	writer: &File,
	reader: &File,
	bytes: u64,
) -> Result<(), InstallFailure<E>> {
	let written = os(fs::fstat(writer))?;
	let read = os(fs::fstat(reader))?;
	if !same_file(&written, &read, bytes) {
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}

fn same_file(first: &fs::Stat, second: &fs::Stat, bytes: u64) -> bool {
	first.st_dev == second.st_dev
		&& first.st_ino == second.st_ino
		&& FileType::from_raw_mode(second.st_mode) == FileType::RegularFile
		&& second.st_nlink == 1
		&& u64::try_from(second.st_size).ok() == Some(bytes)
}

pub(super) fn apply_mode<E: StdError + 'static>(
	fd: impl AsFd,
	mode: u16,
) -> Result<(), InstallFailure<E>> {
	let mode = host_mode(mode);
	os(fs::fchmod(fd.as_fd(), mode))?;
	if Mode::from_raw_mode(os(fs::fstat(fd))?.st_mode) != mode {
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}

pub(super) fn sync<E: StdError + 'static>(fd: impl AsFd) -> Result<(), InstallFailure<E>> {
	os(fs::fsync(fd))
}

pub(super) fn write_link<E: StdError + 'static>(
	parent: &DestinationDirectory<'_>,
	name: &HostName,
	target: &[u8],
	mode: u16,
) -> Result<(), InstallFailure<E>> {
	parent.check_attachment()?;
	let parent = parent.descriptor();
	let name = name.as_str();
	os(fs::symlinkat(
		std::ffi::OsStr::from_bytes(target),
		parent,
		name,
	))?;
	#[cfg(target_os = "macos")]
	os(fs::chmodat(
		parent,
		name,
		host_mode(mode),
		AtFlags::SYMLINK_NOFOLLOW,
	))?;
	let mut buffer = Vec::new();
	buffer.try_reserve_exact(target.len() + 1)?;
	buffer.resize(target.len() + 1, 0);
	let count = os(fs::readlinkat_raw(parent, name, buffer.as_mut_slice()))?;
	let observed = os(fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW))?;
	if buffer.get(..count) != Some(target)
		|| FileType::from_raw_mode(observed.st_mode) != FileType::Symlink
		|| Mode::from_raw_mode(observed.st_mode) != host_mode(mode)
	{
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}

fn host_mode(mode: u16) -> Mode {
	#[cfg(target_os = "linux")]
	let mode = u32::from(mode);
	Mode::from_bits_retain(mode)
}

fn os<T, E: StdError + 'static>(result: rustix::io::Result<T>) -> Result<T, InstallFailure<E>> {
	result.map_err(|error| InstallFailure::Io(error.into()))
}
