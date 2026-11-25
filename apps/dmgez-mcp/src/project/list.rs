use rmcp::schemars::{self, JsonSchema};
use rustix::{
	fs::{self, AtFlags, Dir},
	io::Errno,
};
use serde::Serialize;

use crate::local::{LocalRoot, is_direct_file, same_observation};

use super::project_name;

pub(crate) const MAX_DIRECTORY_ENTRIES: usize = 4096;
pub(crate) const MAX_PAGE_SIZE: usize = 16;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Listing {
	Listed {
		file_names: Vec<String>,
		next_after: Option<String>,
	},
	Unavailable {
		issue: ListIssue,
	},
	Busy,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(crate) enum ListIssue {
	InvalidPageSize,
	InvalidCursor,
	DirectoryUnavailable,
	DirectoryChanged,
	DirectoryScanLimit,
}

impl LocalRoot {
	pub(crate) fn list(&self, after: Option<&str>, page_size: Option<usize>) -> Listing {
		match self.manifest_names(after, page_size.unwrap_or(MAX_PAGE_SIZE)) {
			Ok((file_names, next_after)) => Listing::Listed {
				file_names,
				next_after,
			},
			Err(issue) => Listing::Unavailable { issue },
		}
	}

	fn manifest_names(
		&self,
		after: Option<&str>,
		page_size: usize,
	) -> Result<(Vec<String>, Option<String>), ListIssue> {
		if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
			return Err(ListIssue::InvalidPageSize);
		}
		if after.is_some_and(|name| project_name(name).is_none()) {
			return Err(ListIssue::InvalidCursor);
		}
		let before = fs::fstat(&self.0).map_err(|_| ListIssue::DirectoryUnavailable)?;
		let directory = Dir::read_from(&self.0).map_err(|_| ListIssue::DirectoryUnavailable)?;
		let mut names = Vec::new();
		for (index, entry) in directory.enumerate() {
			if index == MAX_DIRECTORY_ENTRIES {
				return Err(ListIssue::DirectoryScanLimit);
			}
			let entry = entry.map_err(|_| ListIssue::DirectoryUnavailable)?;
			let Ok(name) = entry.file_name().to_str() else {
				continue;
			};
			let Some(path) = project_name(name) else {
				continue;
			};
			if after.is_some_and(|cursor| name <= cursor) {
				continue;
			}
			let stat = fs::statat(&self.0, path, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
				if error == Errno::NOENT {
					ListIssue::DirectoryChanged
				} else {
					ListIssue::DirectoryUnavailable
				}
			})?;
			if is_direct_file(&stat) {
				if stat.st_ino != entry.ino() {
					return Err(ListIssue::DirectoryChanged);
				}
				names.push(name.to_owned());
			}
		}
		let observed = fs::fstat(&self.0).map_err(|_| ListIssue::DirectoryUnavailable)?;
		if !same_observation(&before, &observed) {
			return Err(ListIssue::DirectoryChanged);
		}
		names.sort_unstable();
		let has_more = names.len() > page_size;
		names.truncate(page_size);
		let next = names.last().filter(|_| has_more).cloned();
		Ok((names, next))
	}
}
