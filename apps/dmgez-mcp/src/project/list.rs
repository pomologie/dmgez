use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

use crate::local::{LocalRoot, ScanError};

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
		let mut names = self.matching_names(MAX_DIRECTORY_ENTRIES, |name| {
			project_name(name).is_some() && after.is_none_or(|cursor| name > cursor)
		})?;
		names.sort_unstable();
		let has_more = names.len() > page_size;
		names.truncate(page_size);
		let next = names.last().filter(|_| has_more).cloned();
		Ok((names, next))
	}
}

impl From<ScanError> for ListIssue {
	fn from(error: ScanError) -> Self {
		match error {
			ScanError::Unavailable => Self::DirectoryUnavailable,
			ScanError::Changed => Self::DirectoryChanged,
			ScanError::Limit => Self::DirectoryScanLimit,
		}
	}
}
