use std::{
	fs::{self, File, OpenOptions},
	io::{self, ErrorKind, Read},
	path::{Path, PathBuf},
};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::APP_NAME;

use super::{Change, MAX_ENTRIES, MAX_PATH_BYTES, Preferences};

const FILE_NAME: &str = "recent-files.json";
const LOCK_NAME: &str = "recent-files.lock";
// JSON stores native paths as byte or UTF-16 arrays. A u16 and delimiter occupy at most six bytes.
const MAX_ENCODED_UNIT_BYTES: usize = 6;
const SCHEMA_OVERHEAD_BYTES: usize = 4096;
const MAX_FILE_BYTES: usize =
	MAX_ENTRIES * MAX_PATH_BYTES * MAX_ENCODED_UNIT_BYTES + SCHEMA_OVERHEAD_BYTES;

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error("The local preferences directory is unavailable.")]
	LocalDirectory,
	#[error("The recent-file preferences have an invalid size, type, or schema.")]
	InvalidPreferences,
	#[error("Recent-file preferences contain invalid JSON. {0}")]
	Json(#[from] serde_json::Error),
	#[error(transparent)]
	Io(#[from] io::Error),
}

#[derive(Clone)]
pub(crate) struct Store(PathBuf);

impl Store {
	pub(super) fn local() -> Result<Self, Error> {
		let directory = local_directory()
			.filter(|path| path.is_absolute())
			.ok_or(Error::LocalDirectory)?;
		Ok(Self(directory.join(APP_NAME)))
	}

	pub(super) fn load(&self) -> Result<Preferences, Error> {
		let path = self.0.join(FILE_NAME);
		if !regular_if_present(&path)? {
			return Ok(Preferences::default());
		}
		let file = File::open(&path)?;
		let metadata = file.metadata()?;
		if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
			return Err(Error::InvalidPreferences);
		}
		let mut bytes = Vec::new();
		file.take(MAX_FILE_BYTES as u64 + 1)
			.read_to_end(&mut bytes)?;
		if bytes.len() > MAX_FILE_BYTES {
			return Err(Error::InvalidPreferences);
		}
		let preferences: Preferences = serde_json::from_slice(&bytes)?;
		if !preferences.valid() {
			return Err(Error::InvalidPreferences);
		}
		Ok(preferences)
	}

	pub(super) fn update(&self, change: Change) -> Result<Preferences, Error> {
		fs::create_dir_all(&self.0)?;
		let lock_path = self.0.join(LOCK_NAME);
		regular_if_present(&lock_path)?;
		let lock = OpenOptions::new()
			.read(true)
			.write(true)
			.create(true)
			.truncate(false)
			.open(lock_path)?;
		if !lock.metadata()?.is_file() {
			return Err(Error::InvalidPreferences);
		}
		lock.try_lock().map_err(io::Error::from)?;
		let mut preferences = self.load()?;
		if preferences.apply(change) {
			let stage = NamedTempFile::new_in(&self.0)?;
			serde_json::to_writer(stage.as_file(), &preferences)?;
			stage.as_file().sync_all()?;
			stage
				.persist(self.0.join(FILE_NAME))
				.map_err(|failure| failure.error)?;
		}
		Ok(preferences)
	}
}

fn regular_if_present(path: &Path) -> Result<bool, Error> {
	match fs::symlink_metadata(path) {
		Ok(metadata) if metadata.is_file() => Ok(true),
		Ok(_) => Err(Error::InvalidPreferences),
		Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
		Err(error) => Err(error.into()),
	}
}

#[cfg(target_os = "macos")]
fn local_directory() -> Option<PathBuf> {
	std::env::home_dir().map(|home| home.join("Library").join("Application Support"))
}

#[cfg(target_os = "linux")]
fn local_directory() -> Option<PathBuf> {
	std::env::var_os("XDG_STATE_HOME")
		.map(PathBuf::from)
		.filter(|path| path.is_absolute())
		.or_else(|| std::env::home_dir().map(|home| home.join(".local").join("state")))
}

#[cfg(target_os = "windows")]
fn local_directory() -> Option<PathBuf> {
	std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn local_directory() -> Option<PathBuf> {
	None
}

#[cfg(test)]
impl Store {
	pub(super) fn at(path: PathBuf) -> Self {
		Self(path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn refuses_invalid_or_oversized_state_without_overwriting_it() {
		let directory = tempfile::tempdir().unwrap();
		let store = Store::at(directory.path().to_owned());
		let preferences = directory.path().join(FILE_NAME);
		let invalid = br#"{"schema_version":0,"remember":true,"entries":[]}"#;
		fs::write(&preferences, invalid).unwrap();
		assert!(matches!(store.load(), Err(Error::InvalidPreferences)));
		assert!(matches!(
			store.update(Change::Remember(false)),
			Err(Error::InvalidPreferences)
		));
		assert_eq!(fs::read(&preferences).unwrap(), invalid);
		let oversized = MAX_FILE_BYTES as u64 + 1;
		File::create(&preferences)
			.unwrap()
			.set_len(oversized)
			.unwrap();
		assert!(matches!(store.load(), Err(Error::InvalidPreferences)));
		assert_eq!(fs::metadata(&preferences).unwrap().len(), oversized);
	}
}
