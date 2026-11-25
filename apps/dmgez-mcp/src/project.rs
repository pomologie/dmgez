use std::{
	fs::File,
	io,
	os::fd::OwnedFd,
	path::{Component, Path},
};

use dmgez_config::{ConfigError, Project};
use dmgez_core::source::{CapturedFile, SourceError};
use rmcp::schemars::{self, JsonSchema};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, Stat};
use serde::Serialize;

mod list;
mod read;

pub(super) use list::{Listing, MAX_DIRECTORY_ENTRIES, MAX_PAGE_SIZE};
pub(super) use read::Reading;

pub(super) const MAX_PROJECT_BYTES: usize = 1 << 20;
pub(super) const MAX_FILE_NAME_BYTES: usize = 255;
const FILE_EXTENSION: &str = "toml";
const HOST_ROOT: &str = "/";
const FORBIDDEN_FILE_NAME: [char; 3] = ['/', '\\', ':'];
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::DIRECTORY)
	.union(OFlags::NOFOLLOW)
	.union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::RDONLY
	.union(OFlags::NOFOLLOW)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);

#[derive(Debug)]
pub(super) struct ProjectRoot(OwnedFd);

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Validation {
	Valid { content_entries: usize },
	Invalid { issue: Issue },
	Busy,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(super) enum Issue {
	InvalidFileName,
	ProjectUnavailable,
	ProjectChanged,
	ProjectTooLarge,
	InvalidUtf8,
	InvalidToml { offset: Option<usize> },
	InvalidSchemaVersion,
	UnsupportedSchemaVersion { version: i64 },
	InvalidManifest,
}

impl ProjectRoot {
	pub(super) fn open(root: &Path) -> io::Result<Self> {
		let absolute = std::path::absolute(root)?;
		let mut directory = fs::open(HOST_ROOT, DIRECTORY_FLAGS, Mode::empty())?;
		for component in absolute.components() {
			match component {
				Component::RootDir => (),
				Component::Normal(name) => {
					directory = fs::openat(&directory, name, DIRECTORY_FLAGS, Mode::empty())?;
				}
				_ => return Err(io::ErrorKind::InvalidInput.into()),
			}
		}
		Ok(Self(directory))
	}

	pub(super) fn validate(&self, file_name: &str) -> Validation {
		match self.parse_manifest(file_name) {
			Ok((project, _)) => Validation::Valid {
				content_entries: project.contents.len(),
			},
			Err(issue) => Validation::Invalid { issue },
		}
	}

	fn parse_manifest(&self, file_name: &str) -> Result<(Project, String), Issue> {
		let input = self.read_manifest(file_name)?;
		let project = Project::parse(&input).map_err(config_issue)?;
		Ok((project, input))
	}

	fn read_manifest(&self, file_name: &str) -> Result<String, Issue> {
		let path = project_name(file_name).ok_or(Issue::InvalidFileName)?;
		let named = fs::statat(&self.0, path, AtFlags::SYMLINK_NOFOLLOW)
			.map_err(|_| Issue::ProjectUnavailable)?;
		if !is_direct_manifest(&named) {
			return Err(Issue::ProjectUnavailable);
		}
		let file = fs::openat(&self.0, path, FILE_FLAGS, Mode::empty())
			.map_err(|_| Issue::ProjectUnavailable)?;
		let held = fs::fstat(&file).map_err(|_| Issue::ProjectUnavailable)?;
		if !same_observation(&named, &held) {
			return Err(Issue::ProjectChanged);
		}
		let captured = CapturedFile::new(File::from(file)).map_err(source_issue)?;
		let byte_count = usize::try_from(captured.len())
			.ok()
			.filter(|&size| size <= MAX_PROJECT_BYTES)
			.ok_or(Issue::ProjectTooLarge)?;
		let mut bytes = vec![0; byte_count];
		captured
			.read_exact_at(0, &mut bytes)
			.map_err(source_issue)?;
		let observed = fs::statat(&self.0, path, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
			if error == rustix::io::Errno::NOENT {
				Issue::ProjectChanged
			} else {
				Issue::ProjectUnavailable
			}
		})?;
		if !same_observation(&held, &observed) {
			return Err(Issue::ProjectChanged);
		}
		String::from_utf8(bytes).map_err(|_| Issue::InvalidUtf8)
	}
}

fn project_name(file_name: &str) -> Option<&Path> {
	let path = Path::new(file_name);
	if file_name.len() > MAX_FILE_NAME_BYTES
		|| file_name.contains(FORBIDDEN_FILE_NAME)
		|| file_name.chars().any(char::is_control)
		|| path.extension() != Some(FILE_EXTENSION.as_ref())
	{
		return None;
	}
	Some(path)
}

fn is_direct_manifest(stat: &Stat) -> bool {
	FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1
}

fn same_observation(first: &Stat, second: &Stat) -> bool {
	first.st_dev == second.st_dev
		&& first.st_ino == second.st_ino
		&& first.st_mode == second.st_mode
		&& first.st_nlink == second.st_nlink
		&& first.st_size == second.st_size
		&& first.st_mtime == second.st_mtime
		&& first.st_mtime_nsec == second.st_mtime_nsec
		&& first.st_ctime == second.st_ctime
		&& first.st_ctime_nsec == second.st_ctime_nsec
}

fn source_issue(error: SourceError) -> Issue {
	match error {
		SourceError::Changed => Issue::ProjectChanged,
		_ => Issue::ProjectUnavailable,
	}
}

fn config_issue(error: ConfigError) -> Issue {
	match error {
		ConfigError::Syntax { offset } => Issue::InvalidToml { offset },
		ConfigError::SchemaVersion => Issue::InvalidSchemaVersion,
		ConfigError::UnsupportedVersion(version) => Issue::UnsupportedSchemaVersion { version },
		_ => Issue::InvalidManifest,
	}
}
