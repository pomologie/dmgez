use std::{
	fs::File,
	io,
	os::fd::OwnedFd,
	path::{Component, Path},
};

use dmgez_config::{ConfigError, Project};
use dmgez_core::source::{CapturedFile, SourceError};
use rmcp::schemars::{self, JsonSchema};
use rustix::fs::{self, Mode, OFlags};
use serde::Serialize;

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
		match self
			.read(file_name)
			.and_then(|input| Project::parse(&input).map_err(config_issue))
		{
			Ok(project) => Validation::Valid {
				content_entries: project.contents.len(),
			},
			Err(issue) => Validation::Invalid { issue },
		}
	}

	fn read(&self, file_name: &str) -> Result<String, Issue> {
		let path = Path::new(file_name);
		if file_name.len() > MAX_FILE_NAME_BYTES
			|| file_name.contains(FORBIDDEN_FILE_NAME)
			|| file_name.chars().any(char::is_control)
			|| path.extension() != Some(FILE_EXTENSION.as_ref())
		{
			return Err(Issue::InvalidFileName);
		}
		let file = fs::openat(&self.0, path, FILE_FLAGS, Mode::empty())
			.map_err(|_| Issue::ProjectUnavailable)?;
		if fs::fstat(&file)
			.map_err(|_| Issue::ProjectUnavailable)?
			.st_nlink != 1
		{
			return Err(Issue::ProjectUnavailable);
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
		String::from_utf8(bytes).map_err(|_| Issue::InvalidUtf8)
	}
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
