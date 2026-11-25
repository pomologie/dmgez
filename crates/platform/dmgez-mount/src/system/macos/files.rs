use std::{
	fs::{self, File},
	os::{fd::OwnedFd, unix::ffi::OsStrExt},
	path::{Component, Path},
};

use dmgez_udif::reader::{FileSource, Image, Limits};
use dmgez_vfs::{Disk, Filesystem, Layout};
use rustix::fs::{
	AtFlags, Dir, FileType, Mode, OFlags, Stat, StatVfsMountFlags, fstat, fstatfs, fstatvfs, open,
	openat, statat,
};

use super::super::{
	Error,
	record::{Device, DeviceStamp, FileStamp, Identity, MountedIdentity, Timestamp},
};

const ROOT: &str = "/";
const DEV_DIRECTORY: &str = "/dev";
const HFS_TYPE: &str = "hfs";
const CURRENT_DIRECTORY: &[u8] = b".";
const PARENT_DIRECTORY: &[u8] = b"..";

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const SOURCE_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::NOCTTY)
	.union(OFlags::CLOEXEC);

pub(super) struct HeldImage {
	file: File,
	pub(super) stamp: FileStamp,
}

impl HeldImage {
	pub(super) fn open(path: &Path) -> Result<Self, Error> {
		let (parent, name) = open_parent(path)?;
		// Observe the type before opening any file that could be a device or FIFO.
		let named =
			statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| io(path, error))?;
		if FileType::from_raw_mode(named.st_mode) != FileType::RegularFile {
			return Err(Error::NotRegular { path: path.into() });
		}
		let file =
			openat(&parent, name, SOURCE_FLAGS, Mode::empty()).map_err(|error| io(path, error))?;
		let held = fstat(&file).map_err(|error| io(path, error))?;
		let stamp = file_stamp(&held);
		if FileType::from_raw_mode(held.st_mode) != FileType::RegularFile
			|| file_stamp(&named) != stamp
		{
			return Err(Error::IdentityMismatch);
		}
		Ok(Self {
			file: file.into(),
			stamp,
		})
	}

	pub(super) fn verify(&self, path: &Path) -> Result<(), Error> {
		let held = fstat(&self.file).map_err(|error| io(path, error))?;
		let named = named_state(path)?;
		if file_stamp(&held) != self.stamp || file_stamp(&named) != self.stamp {
			return Err(Error::IdentityMismatch);
		}
		Ok(())
	}

	pub(super) fn admit_format(&self, path: &Path) -> Result<(), Error> {
		let file = self.file.try_clone().map_err(|error| io(path, error))?;
		let source = FileSource::new(file).map_err(|_| Error::UnsupportedImage)?;
		let mut image =
			Image::open(source, Limits::default()).map_err(|_| Error::UnsupportedImage)?;
		let disk = Disk::discover(&mut image, dmgez_vfs::Limits::default())
			.map_err(|_| Error::UnsupportedImage)?;
		if !matches!(
			disk.layout(),
			Layout::Partitionless {
				filesystem: Filesystem::HfsPlus,
				..
			}
		) {
			return Err(Error::UnsupportedImage);
		}
		// Verify all declared UDIF checksums here so hdiutil need not write its
		// verification-cache attribute to the source.
		image.verify().map_err(|_| Error::UnsupportedImage)?;
		Ok(())
	}
}

pub(super) struct HeldDirectory {
	fd: OwnedFd,
	pub(super) id: Identity,
}

impl HeldDirectory {
	pub(super) fn open(path: &Path) -> Result<Self, Error> {
		let (parent, name) = open_parent(path)?;
		let fd = openat(&parent, name, DIRECTORY_FLAGS, Mode::empty())
			.map_err(|error| io(path, error))?;
		let id = identity(&fstat(&fd).map_err(|error| io(path, error))?);
		Ok(Self { fd, id })
	}

	pub(super) fn is_mountpoint(&self, path: &Path) -> Result<bool, Error> {
		let filesystem = fstatfs(&self.fd).map_err(|error| io(path, error))?;
		Ok(native_name_equals(&filesystem.f_mntonname, path))
	}

	pub(super) fn verify_empty(&self, path: &Path) -> Result<(), Error> {
		if self.is_mountpoint(path)? {
			return Err(Error::MountpointOccupied { path: path.into() });
		}
		for entry in Dir::read_from(&self.fd).map_err(|error| io(path, error))? {
			let entry = entry.map_err(|error| io(path, error))?;
			if !matches!(
				entry.file_name().to_bytes(),
				CURRENT_DIRECTORY | PARENT_DIRECTORY
			) {
				return Err(Error::MountpointOccupied { path: path.into() });
			}
		}
		self.verify_name(path)
	}

	pub(super) fn verify_name(&self, path: &Path) -> Result<(), Error> {
		if identity(&named_state(path)?) != self.id {
			return Err(Error::IdentityMismatch);
		}
		Ok(())
	}

	pub(super) fn mounted_identity(
		&self,
		path: &Path,
		device: &Device,
	) -> Result<Option<MountedIdentity>, Error> {
		let mount = fstatfs(&self.fd).map_err(|error| io(path, error))?;
		let volume = fstatvfs(&self.fd).map_err(|error| io(path, error))?;
		if !native_name_equals(&mount.f_mntonname, path)
			|| !native_name_equals(&mount.f_mntfromname, Path::new(device.as_str()))
			|| !native_name_equals(&mount.f_fstypename, Path::new(HFS_TYPE))
			|| !volume.f_flag.contains(StatVfsMountFlags::RDONLY)
		{
			return Ok(None);
		}
		self.verify_name(path)?;
		Ok(Some(MountedIdentity {
			root: self.id,
			fsid: volume.f_fsid,
		}))
	}
}

pub(super) fn exact_path(path: &Path) -> Result<(), Error> {
	let canonical = fs::canonicalize(path).map_err(|error| io(path, error))?;
	if canonical.as_os_str() != path.as_os_str() {
		return Err(Error::Path { path: path.into() });
	}
	Ok(())
}

pub(super) fn named_image_matches(path: &Path, expected: FileStamp) -> Result<bool, Error> {
	let named = named_state(path)?;
	Ok(
		FileType::from_raw_mode(named.st_mode) == FileType::RegularFile
			&& file_stamp(&named) == expected,
	)
}

pub(super) fn image_alias(path: &Path, expected: Identity) -> bool {
	use std::os::unix::fs::MetadataExt;

	fs::metadata(path).is_ok_and(|metadata| {
		metadata.is_file()
			&& metadata.ino() == expected.inode
			&& metadata.dev() == u64::from(expected.device.cast_unsigned())
	})
}

pub(super) fn device_stamp(device: &Device) -> Result<Option<DeviceStamp>, Error> {
	let path = Path::new(device.as_str());
	let dev = open(DEV_DIRECTORY, DIRECTORY_FLAGS, Mode::empty())
		.map_err(|error| io(Path::new(DEV_DIRECTORY), error))?;
	let basename = path.file_name().ok_or(Error::InvalidEvidence)?;
	match statat(&dev, basename, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::BlockDevice => {
			Ok(Some(DeviceStamp {
				id: identity(&stat),
				rdev: stat.st_rdev,
				changed: timestamp(stat.st_ctime, stat.st_ctime_nsec),
			}))
		}
		Ok(_) => Err(Error::IdentityMismatch),
		Err(rustix::io::Errno::NOENT) => Ok(None),
		Err(error) => Err(io(path, error)),
	}
}

fn named_state(path: &Path) -> Result<Stat, Error> {
	let (parent, name) = open_parent(path)?;
	statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| io(path, error))
}

fn open_parent(path: &Path) -> Result<(OwnedFd, &std::ffi::OsStr), Error> {
	let parent = path
		.parent()
		.ok_or_else(|| Error::Path { path: path.into() })?;
	let name = path
		.file_name()
		.ok_or_else(|| Error::Path { path: path.into() })?;
	let mut head =
		open(ROOT, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| io(Path::new(ROOT), error))?;
	let mut components = parent.components();
	if components.next() != Some(Component::RootDir) {
		return Err(Error::Path { path: path.into() });
	}
	for component in components {
		let Component::Normal(segment) = component else {
			return Err(Error::Path { path: path.into() });
		};
		head = openat(&head, segment, DIRECTORY_FLAGS, Mode::empty())
			.map_err(|error| io(path, error))?;
	}
	Ok((head, name))
}

fn native_name_equals(native: &[i8], path: &Path) -> bool {
	native
		.iter()
		.take_while(|&&byte| byte != 0)
		.map(|&byte| byte.cast_unsigned())
		.eq(path.as_os_str().as_bytes().iter().copied())
}

fn identity(stat: &Stat) -> Identity {
	Identity {
		device: stat.st_dev,
		inode: stat.st_ino,
		created: timestamp(stat.st_birthtime, stat.st_birthtime_nsec),
	}
}

fn file_stamp(stat: &Stat) -> FileStamp {
	FileStamp {
		id: identity(stat),
		length: stat.st_size,
		modified: timestamp(stat.st_mtime, stat.st_mtime_nsec),
		changed: timestamp(stat.st_ctime, stat.st_ctime_nsec),
	}
}

const fn timestamp(seconds: i64, nanos: i64) -> Timestamp {
	Timestamp { seconds, nanos }
}

pub(super) fn io(path: &Path, source: impl Into<std::io::Error>) -> Error {
	Error::Io {
		path: path.into(),
		source: source.into(),
	}
}

#[cfg(test)]
mod tests;
