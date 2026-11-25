use std::{
	fs::{self, File},
	io,
	path::{Path, PathBuf},
};

use same_file::Handle;
use tempfile::{Builder, NamedTempFile, TempDir};

use crate::SignError;

use super::{
	artifact::{Artifact, check_file_name, image_path},
	quarantine,
};

const STAGE_PREFIX: &str = ".dmgez-signing-";
const IMAGE_SUFFIX: &str = ".dmg";
const CHANGED_DIRECTORY: &str = "the signing directory changed";

pub(super) struct Stage {
	file: NamedTempFile,
	identity: Handle,
	directory_identity: Handle,
	parent_identity: Handle,
	directory: TempDir,
	destination: PathBuf,
}

impl Stage {
	pub(super) fn new(destination: &Path) -> Result<Self, SignError> {
		let destination = image_path(destination).map_err(stage_error)?;
		match fs::symlink_metadata(&destination) {
			Ok(_) => return Err(stage_error(io::ErrorKind::AlreadyExists.into())),
			Err(error) if error.kind() == io::ErrorKind::NotFound => (),
			Err(error) => return Err(stage_error(error)),
		}
		let parent = parent_of(&destination)?;
		let parent_identity = open_directory(parent).map_err(stage_error)?;
		let directory = Builder::new()
			.prefix(STAGE_PREFIX)
			.tempdir_in(parent)
			.map_err(stage_error)?;
		let directory_identity = open_directory(directory.path()).map_err(stage_error)?;
		let file = Builder::new()
			.suffix(IMAGE_SUFFIX)
			.tempfile_in(directory.path())
			.map_err(stage_error)?;
		let identity = Handle::from_file(file.as_file().try_clone().map_err(stage_error)?)
			.map_err(stage_error)?;
		let stage = Self {
			file,
			identity,
			directory_identity,
			parent_identity,
			directory,
			destination,
		};
		stage.validate()?;
		Ok(stage)
	}

	pub(super) fn file_mut(&mut self) -> &mut File {
		self.file.as_file_mut()
	}

	pub(super) fn write_quarantine(&self, value: Option<&[u8]>) -> Result<(), SignError> {
		#[cfg(windows)]
		if value.is_some() {
			quarantine::check_rename(self.directory.path())?;
		}
		quarantine::write(self.file.as_file(), self.file.path(), value)
	}

	pub(super) fn sync(&self) -> Result<(), SignError> {
		self.file.as_file().sync_all().map_err(stage_error)
	}

	pub(super) fn artifact(&self) -> Result<Artifact, SignError> {
		Artifact::from_file(
			self.file.path(),
			self.file.as_file().try_clone().map_err(stage_error)?,
		)
	}

	pub(super) fn validate(&self) -> Result<(), SignError> {
		let parent = open_directory(parent_of(&self.destination)?).map_err(stage_error)?;
		let directory = open_directory(self.directory.path()).map_err(stage_error)?;
		if parent != self.parent_identity || directory != self.directory_identity {
			return Err(stage_error(io::Error::new(
				io::ErrorKind::InvalidData,
				CHANGED_DIRECTORY,
			)));
		}
		check_file_name(&self.identity, self.file.path()).map_err(stage_error)
	}

	pub(super) fn publish(self) -> Result<PathBuf, SignError> {
		self.validate()?;
		self.file
			.persist_noclobber(&self.destination)
			.map_err(|failure| stage_error(failure.error))?;
		Ok(self.destination)
	}
}

fn parent_of(path: &Path) -> Result<&Path, SignError> {
	path.parent()
		.ok_or_else(|| stage_error(io::ErrorKind::InvalidInput.into()))
}

fn open_directory(path: &Path) -> io::Result<Handle> {
	if !fs::symlink_metadata(path)?.is_dir() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			CHANGED_DIRECTORY,
		));
	}
	Handle::from_path(path)
}

fn stage_error(error: io::Error) -> SignError {
	SignError::Stage(Box::new(error))
}
