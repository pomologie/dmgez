//! Private, same-directory staging for a path-based native signer.

use std::{
	ffi::{OsStr, OsString},
	fs::{self, File},
	io,
	path::{Component, Path, PathBuf},
};

use rustix::{
	fd::OwnedFd,
	fs::{self as unix, AtFlags, FileType, Mode, OFlags, RenameFlags, Stat},
	io::Errno,
};
use thiserror::Error;

#[cfg(test)]
mod tests;

const ROOT_DIRECTORY: &str = "/";
const CURRENT_DIRECTORY: &str = ".";
const PARENT_DIRECTORY: &[u8] = b"..";
const CURRENT_DIRECTORY_BYTES: &[u8] = b".";
const PATH_SEPARATOR: u8 = b'/';
const IMAGE_EXTENSION: &str = "dmg";
const STAGE_PREFIX: &str = ".dmgez-signing-";
const STAGE_SUFFIX: &str = ".stage";
const STAGED_IMAGE: &str = "image.dmg";
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const PRIVATE_DIRECTORY_MODE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const PRIVATE_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);
const CREATE_FLAGS: OFlags = OFlags::RDWR
	.union(OFlags::CREATE)
	.union(OFlags::EXCL)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);

#[derive(Debug, Error)]
pub(super) enum Error {
	#[error("signed output must name a .dmg file")]
	InvalidDestination,
	#[error("could not open the output directory")]
	Directory(#[source] io::Error),
	#[error("could not inspect the signed image output or stage")]
	Inspect(#[source] io::Error),
	#[error("signed output already exists")]
	AlreadyExists,
	#[error("could not create a private signing stage")]
	CreateStage(#[source] io::Error),
	#[error("could not open the staged signing image")]
	OpenImage(#[source] io::Error),
	#[error("the output directory changed during signing")]
	DirectoryChanged,
	#[error("the staging directory or its image changed during signing")]
	StageChanged,
	#[error("could not sync the verified signing image")]
	Sync(#[source] io::Error),
	#[error("could not publish the verified signing image")]
	Commit(#[source] io::Error),
}

/// Owns the private stage and holds stable descriptors for both directories.
///
/// The native signer needs an absolute pathname and can replace its input
/// during signing. Use a trusted local output parent whose namespace remains
/// stable while the native tool runs. The final rename never replaces an
/// existing entry. The signed file is synced before publication.
pub(super) struct Stage {
	output_directory: OwnedFd,
	output_path: PathBuf,
	output_name: OsString,
	stage_directory: OwnedFd,
	stage_name: OsString,
	image_path: PathBuf,
}

impl Stage {
	pub(super) fn new(destination: &Path) -> Result<Self, Error> {
		let (parent, output_name) = output_parts(destination)?;
		let parent = fs::canonicalize(parent).map_err(Error::Directory)?;
		let output_directory =
			open_directory(&parent).map_err(|error| Error::Directory(error.into()))?;
		match stat_entry(&output_directory, output_name) {
			Ok(_) => return Err(Error::AlreadyExists),
			Err(Errno::NOENT) => {}
			Err(error) => return Err(inspect(error)),
		}

		let named = tempfile::Builder::new()
			.prefix(STAGE_PREFIX)
			.suffix(STAGE_SUFFIX)
			.disable_cleanup(true)
			.make_in(&parent, |path| create_directory(&output_directory, path))
			.map_err(Error::CreateStage)?;
		let ((stage_directory, stage_name), _) = named.into_parts();
		let image_path = parent.join(&stage_name).join(STAGED_IMAGE);
		let stage = Self {
			output_directory,
			output_path: parent.join(output_name),
			output_name: output_name.to_os_string(),
			stage_directory,
			stage_name,
			image_path,
		};
		stage.validate()?;
		Ok(stage)
	}

	pub(super) fn path(&self) -> &Path {
		&self.image_path
	}

	pub(super) fn destination(&self) -> &Path {
		&self.output_path
	}

	pub(super) fn create_image(&self) -> Result<File, Error> {
		self.validate()?;
		unix::openat(
			&self.stage_directory,
			STAGED_IMAGE,
			CREATE_FLAGS,
			PRIVATE_FILE_MODE,
		)
		.map(File::from)
		.map_err(|error| Error::OpenImage(error.into()))
	}

	/// Rechecks the path that will be passed to the native signer.
	pub(super) fn validate(&self) -> Result<(), Error> {
		let output_parent = self.output_path.parent().ok_or(Error::InvalidDestination)?;
		let current_parent = match open_directory(output_parent) {
			Ok(directory) => unix::fstat(directory).map_err(inspect)?,
			Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP) => return Err(Error::DirectoryChanged),
			Err(error) => return Err(Error::Directory(error.into())),
		};
		if !same_identity(
			&unix::fstat(&self.output_directory).map_err(inspect)?,
			&current_parent,
		) {
			return Err(Error::DirectoryChanged);
		}
		let current_stage = match stat_entry(&self.output_directory, &self.stage_name) {
			Ok(current) => current,
			Err(Errno::NOENT) => return Err(Error::StageChanged),
			Err(error) => return Err(inspect(error)),
		};
		if !same_identity(
			&unix::fstat(&self.stage_directory).map_err(inspect)?,
			&current_stage,
		) || !is_private_directory(&current_stage)
		{
			return Err(Error::StageChanged);
		}
		Ok(())
	}

	pub(super) fn validate_image(&self, file: &File) -> Result<(), Error> {
		let expected = unix::fstat(file).map_err(inspect)?;
		if !is_private_file(&expected) {
			return Err(Error::StageChanged);
		}
		self.validate()?;
		let current = match stat_entry(&self.stage_directory, OsStr::new(STAGED_IMAGE)) {
			Ok(current) => current,
			Err(Errno::NOENT) => return Err(Error::StageChanged),
			Err(error) => return Err(inspect(error)),
		};
		if !same_stamp(&expected, &current) {
			return Err(Error::StageChanged);
		}
		Ok(())
	}

	pub(super) fn publish(self, verified: &File) -> Result<PathBuf, Error> {
		verified.sync_all().map_err(Error::Sync)?;
		self.validate_image(verified)?;
		match unix::renameat_with(
			&self.stage_directory,
			STAGED_IMAGE,
			&self.output_directory,
			&self.output_name,
			RenameFlags::NOREPLACE,
		) {
			Ok(()) => Ok(self.destination().to_path_buf()),
			Err(Errno::EXIST) => Err(Error::AlreadyExists),
			Err(error) => Err(Error::Commit(error.into())),
		}
	}
}

impl Drop for Stage {
	fn drop(&mut self) {
		// The child slot belongs to this directory descriptor. No recursive
		// pathname cleanup can touch an unrelated replacement stage.
		let _ = unix::unlinkat(&self.stage_directory, STAGED_IMAGE, AtFlags::empty());
		if let Ok(original) = unix::fstat(&self.stage_directory)
			&& let Ok(current) = stat_entry(&self.output_directory, &self.stage_name)
			&& same_identity(&original, &current)
		{
			let _ = unix::unlinkat(&self.output_directory, &self.stage_name, AtFlags::REMOVEDIR);
		}
	}
}

fn output_parts(destination: &Path) -> Result<(&Path, &OsStr), Error> {
	match destination
		.as_os_str()
		.as_encoded_bytes()
		.rsplit(|&byte| byte == PATH_SEPARATOR)
		.next()
	{
		Some(b"" | CURRENT_DIRECTORY_BYTES | PARENT_DIRECTORY) | None => {
			return Err(Error::InvalidDestination);
		}
		_ => {}
	}
	if !destination
		.extension()
		.is_some_and(|extension| extension.eq_ignore_ascii_case(IMAGE_EXTENSION))
	{
		return Err(Error::InvalidDestination);
	}
	let name = destination.file_name().ok_or(Error::InvalidDestination)?;
	let parent = destination
		.parent()
		.filter(|path| !path.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new(CURRENT_DIRECTORY));
	Ok((parent, name))
}

fn open_directory(path: &Path) -> rustix::io::Result<OwnedFd> {
	let mut directory = unix::open(ROOT_DIRECTORY, DIRECTORY_FLAGS, Mode::empty())?;
	for component in path.components() {
		if component != Component::RootDir {
			directory = unix::openat(
				directory,
				component.as_os_str(),
				DIRECTORY_FLAGS,
				Mode::empty(),
			)?;
		}
	}
	Ok(directory)
}

fn create_directory(parent: &OwnedFd, path: &Path) -> io::Result<(OwnedFd, OsString)> {
	let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
	unix::mkdirat(parent, name, PRIVATE_DIRECTORY_MODE)?;
	match unix::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
		Ok(stage) => Ok((stage, name.to_os_string())),
		Err(error) => {
			let _ = unix::unlinkat(parent, name, AtFlags::REMOVEDIR);
			Err(error.into())
		}
	}
}

fn stat_entry(directory: &OwnedFd, name: &OsStr) -> rustix::io::Result<Stat> {
	unix::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
}

fn inspect(error: Errno) -> Error {
	Error::Inspect(error.into())
}

fn is_private_directory(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::Directory
		&& Mode::from_raw_mode(stat.st_mode) == PRIVATE_DIRECTORY_MODE
}

fn is_private_file(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1
}

fn same_identity(expected: &Stat, current: &Stat) -> bool {
	(expected.st_dev, expected.st_ino) == (current.st_dev, current.st_ino)
}

fn same_stamp(expected: &Stat, current: &Stat) -> bool {
	same_identity(expected, current)
		&& expected.st_mode == current.st_mode
		&& expected.st_size == current.st_size
		&& expected.st_nlink == current.st_nlink
		&& (expected.st_mtime, expected.st_mtime_nsec) == (current.st_mtime, current.st_mtime_nsec)
		&& (expected.st_ctime, expected.st_ctime_nsec) == (current.st_ctime, current.st_ctime_nsec)
}
