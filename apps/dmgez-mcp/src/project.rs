use std::path::Path;

use dmgez_config::{ConfigError, Project};
use dmgez_core::source::{CapturedFile, SourceError};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

use crate::local::{FileAdmission, LocalRoot, direct_name};

mod list;
mod read;
mod update;

pub(super) use list::{Listing, MAX_DIRECTORY_ENTRIES, MAX_PAGE_SIZE};
pub(super) use read::Reading;
pub(super) use update::{MAX_PROJECT_EDITS, Update, UpdateRequest};

pub(super) const MAX_PROJECT_BYTES: usize = 1 << 20;
const FILE_EXTENSION: &str = "toml";

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

impl LocalRoot {
	pub(super) fn validate(&self, file_name: &str) -> Validation {
		match self.parse_manifest(file_name) {
			Ok((project, _)) => Validation::Valid {
				content_entries: project.contents.len(),
			},
			Err(issue) => Validation::Invalid { issue },
		}
	}

	fn parse_manifest(&self, file_name: &str) -> Result<(Project, String), Issue> {
		let input =
			String::from_utf8(self.read_manifest(file_name)?).map_err(|_| Issue::InvalidUtf8)?;
		let project = Project::parse(&input).map_err(config_issue)?;
		Ok((project, input))
	}

	fn read_manifest(&self, file_name: &str) -> Result<Vec<u8>, Issue> {
		let path = project_name(file_name).ok_or(Issue::InvalidFileName)?;
		let (file, binding) = self.open_file(path).map_err(file_issue)?;
		let captured = CapturedFile::new(file).map_err(source_issue)?;
		let byte_count = usize::try_from(captured.len())
			.ok()
			.filter(|&size| size <= MAX_PROJECT_BYTES)
			.ok_or(Issue::ProjectTooLarge)?;
		let mut bytes = vec![0; byte_count];
		captured
			.read_exact_at(0, &mut bytes)
			.map_err(source_issue)?;
		binding.verify().map_err(file_issue)?;
		Ok(bytes)
	}
}

fn project_name(file_name: &str) -> Option<&Path> {
	direct_name(file_name, FILE_EXTENSION)
}

fn file_issue(error: FileAdmission) -> Issue {
	match error {
		FileAdmission::Unavailable => Issue::ProjectUnavailable,
		FileAdmission::Changed => Issue::ProjectChanged,
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
