//! Private, same-directory staging for a path-based native signer.

use std::{
	cell::Cell,
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

mod cleanup;

#[cfg(test)]
mod tests;

const ROOT_DIRECTORY: &str = "/";
const CURRENT_DIRECTORY: &str = ".";
const PARENT_DIRECTORY: &[u8] = b"..";
const CURRENT_DIRECTORY_BYTES: &[u8] = b".";
const PATH_SEPARATOR: u8 = b'/';
const IMAGE_EXTENSION: &str = "dmg";
const APP_EXTENSION: &str = "app";
const STAGE_PREFIX: &str = ".dmgez-signing-";
const STAGE_SUFFIX: &str = ".stage";
const STAGED_IMAGE: &str = "image.dmg";
const STAGED_APP: &str = "Application.app";
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
	#[error("signed output must name a .dmg file or .app bundle of the requested kind")]
	InvalidDestination,
	#[error("could not open the source bundle directory")]
	SourceDirectory(#[source] io::Error),
	#[error("signed output cannot be inside the source bundle")]
	InsideSource,
	#[error("could not open the output directory")]
	Directory(#[source] io::Error),
	#[error("could not inspect the signed output or stage")]
	Inspect(#[source] io::Error),
	#[error("signed output already exists")]
	AlreadyExists,
	#[error("could not create a private signing stage")]
	CreateStage(#[source] io::Error),
	#[error("could not open the staged signing image")]
	OpenImage(#[source] io::Error),
	#[error("the output directory changed during signing")]
	DirectoryChanged,
	#[error("the staging directory or its artifact changed during signing")]
	StageChanged,
	#[error("could not sync the verified signing artifact")]
	Sync(#[source] io::Error),
	#[error("could not publish the verified signing artifact")]
	Commit(#[source] io::Error),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
	Image,
	App,
}

impl Kind {
	fn extension(self) -> &'static str {
		match self {
			Self::Image => IMAGE_EXTENSION,
			Self::App => APP_EXTENSION,
		}
	}

	fn slot(self) -> &'static str {
		match self {
			Self::Image => STAGED_IMAGE,
			Self::App => STAGED_APP,
		}
	}
}

/// Owns the private stage and holds stable descriptors for both directories.
///
/// The native signer needs an absolute pathname and can replace its input
/// during signing. Use a trusted local output parent whose namespace remains
/// stable while the native tool runs. The final rename never replaces an
/// existing entry. The verified artifact is synced before publication.
pub(super) struct Stage {
	output_directory: OwnedFd,
	output_path: PathBuf,
	output_name: OsString,
	stage_directory: OwnedFd,
	stage_name: OsString,
	staged_path: PathBuf,
	kind: Kind,
	app_root: Cell<Option<Stat>>,
}

impl Stage {
	pub(super) fn new(destination: &Path) -> Result<Self, Error> {
		Self::create(destination, Kind::Image, None)
	}

	pub(super) fn new_app(destination: &Path, source: &Path) -> Result<Self, Error> {
		let source = fs::canonicalize(source).map_err(Error::SourceDirectory)?;
		let directory = open_directory(&source)
			.and_then(unix::fstat)
			.map_err(|error| Error::SourceDirectory(error.into()))?;
		Self::create(destination, Kind::App, Some(&directory))
	}

	fn create(destination: &Path, kind: Kind, source: Option<&Stat>) -> Result<Self, Error> {
		let (parent, output_name) = output_parts(destination, kind.extension())?;
		let parent = fs::canonicalize(parent).map_err(Error::Directory)?;
		let output_directory =
			open_directory(&parent).map_err(|error| Error::Directory(error.into()))?;
		if let Some(source) = source
			&& has_ancestor(&output_directory, source).map_err(inspect)?
		{
			return Err(Error::InsideSource);
		}
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
		let staged_path = parent.join(&stage_name).join(kind.slot());
		let stage = Self {
			output_directory,
			output_path: parent.join(output_name),
			output_name: output_name.to_os_string(),
			stage_directory,
			stage_name,
			staged_path,
			kind,
			app_root: Cell::new(None),
		};
		stage.validate()?;
		Ok(stage)
	}

	pub(super) fn path(&self) -> &Path {
		&self.staged_path
	}

	pub(super) fn destination(&self) -> &Path {
		&self.output_path
	}

	pub(super) fn create_image(&self) -> Result<File, Error> {
		self.require_kind(Kind::Image)?;
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
		self.require_kind(Kind::Image)?;
		let expected = unix::fstat(file).map_err(inspect)?;
		if !is_private_file(&expected) {
			return Err(Error::StageChanged);
		}
		self.validate_artifact(&expected)
	}

	pub(super) fn validate_app(&self, directory: &File) -> Result<(), Error> {
		self.require_kind(Kind::App)?;
		let expected = unix::fstat(directory).map_err(inspect)?;
		if FileType::from_raw_mode(expected.st_mode) != FileType::Directory
			|| self
				.app_root
				.get()
				.is_some_and(|root| !same_identity(&root, &expected))
		{
			return Err(Error::StageChanged);
		}
		self.validate_artifact(&expected)?;
		self.app_root.set(Some(expected));
		Ok(())
	}

	fn require_kind(&self, kind: Kind) -> Result<(), Error> {
		if self.kind != kind {
			return Err(Error::StageChanged);
		}
		Ok(())
	}

	fn validate_artifact(&self, expected: &Stat) -> Result<(), Error> {
		self.validate()?;
		let current = match stat_entry(&self.stage_directory, OsStr::new(self.kind.slot())) {
			Ok(current) => current,
			Err(Errno::NOENT) => return Err(Error::StageChanged),
			Err(error) => return Err(inspect(error)),
		};
		if !same_stamp(expected, &current) {
			return Err(Error::StageChanged);
		}
		Ok(())
	}

	pub(super) fn publish(self, verified: &File) -> Result<PathBuf, Error> {
		verified.sync_all().map_err(Error::Sync)?;
		self.validate_image(verified)?;
		self.rename()
	}

	pub(super) fn publish_app(self, verified: &File) -> Result<PathBuf, Error> {
		self.validate_app(verified)?;
		verified.sync_all().map_err(Error::Sync)?;
		self.validate_app(verified)?;
		self.rename()
	}

	fn rename(&self) -> Result<PathBuf, Error> {
		match unix::renameat_with(
			&self.stage_directory,
			self.kind.slot(),
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
		match self.kind {
			Kind::Image => {
				let _ = unix::unlinkat(&self.stage_directory, STAGED_IMAGE, AtFlags::empty());
			}
			Kind::App => cleanup::remove_app(
				&self.stage_directory,
				OsStr::new(STAGED_APP),
				self.app_root.get().as_ref(),
			),
		}
		if let Ok(original) = unix::fstat(&self.stage_directory)
			&& let Ok(current) = stat_entry(&self.output_directory, &self.stage_name)
			&& same_identity(&original, &current)
		{
			let _ = unix::unlinkat(&self.output_directory, &self.stage_name, AtFlags::REMOVEDIR);
		}
	}
}

fn output_parts<'a>(
	destination: &'a Path,
	extension: &str,
) -> Result<(&'a Path, &'a OsStr), Error> {
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
		.is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
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

fn has_ancestor(directory: &OwnedFd, source: &Stat) -> rustix::io::Result<bool> {
	let mut current = unix::openat(directory, CURRENT_DIRECTORY, DIRECTORY_FLAGS, Mode::empty())?;
	let mut identity = unix::fstat(&current)?;
	loop {
		if same_identity(&identity, source) {
			return Ok(true);
		}
		let parent = unix::openat(&current, PARENT_DIRECTORY, DIRECTORY_FLAGS, Mode::empty())?;
		let parent_identity = unix::fstat(&parent)?;
		if same_identity(&identity, &parent_identity) {
			return Ok(false);
		}
		current = parent;
		identity = parent_identity;
	}
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
