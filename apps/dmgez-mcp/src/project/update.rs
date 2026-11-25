use dmgez_config::project_edit::{ProjectDocument, ProjectEditError};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

use crate::local::LocalRoot;

use super::{Issue, MAX_PROJECT_BYTES, config_issue, read::revision_sha256};

mod request;

pub(crate) use request::{MAX_PROJECT_EDITS, UpdateRequest};

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Update {
	Planned {
		source: Revision,
		proposed: Revision,
		requested_edits: usize,
		difference: Option<SourceDifference>,
	},
	StaleRevision {
		current_revision_sha256: String,
	},
	Unavailable {
		issue: Issue,
	},
	Rejected {
		issue: EditIssue,
	},
	Busy,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct Revision {
	revision_sha256: String,
	manifest_bytes: usize,
	content_entries: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SourceDifference {
	/// Common UTF-8 prefix length, which is also the start of the changed span.
	start_byte: usize,
	/// Bytes in that span in the admitted manifest.
	replaced_bytes: usize,
	/// Bytes in the same span in the proposed manifest.
	replacement_bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(crate) enum EditIssue {
	UnknownTarget { edit_index: usize },
	ConflictingEdit { edit_index: usize },
	InvalidResult { issue: Issue },
	ProposedManifestTooLarge,
	EditFailed,
}

impl LocalRoot {
	pub(crate) fn plan_update(&self, request: UpdateRequest) -> Update {
		match self.read_manifest(request.file_name()) {
			Ok(bytes) => plan(bytes, request),
			Err(issue) => Update::Unavailable { issue },
		}
	}
}

fn plan(bytes: Vec<u8>, request: UpdateRequest) -> Update {
	let source_sha256 = revision_sha256(&bytes);
	if source_sha256 != request.expected_revision_sha256() {
		return Update::StaleRevision {
			current_revision_sha256: source_sha256,
		};
	}
	let Ok(source) = String::from_utf8(bytes) else {
		return Update::Unavailable {
			issue: Issue::InvalidUtf8,
		};
	};
	let mut document = match ProjectDocument::open(&source) {
		Ok(document) => document,
		Err(error) => {
			return Update::Unavailable {
				issue: open_issue(error),
			};
		}
	};
	let source_revision = Revision {
		revision_sha256: source_sha256,
		manifest_bytes: source.len(),
		content_entries: document.project().contents.len(),
	};
	let edits = request.into_edits();
	let changed = match document.apply(&edits) {
		Ok(changed) => changed,
		Err(error) => {
			return Update::Rejected {
				issue: error.into(),
			};
		}
	};
	let proposed = document.source();
	if proposed.len() > MAX_PROJECT_BYTES {
		return Update::Rejected {
			issue: EditIssue::ProposedManifestTooLarge,
		};
	}
	Update::Planned {
		proposed: Revision {
			revision_sha256: revision_sha256(proposed.as_bytes()),
			manifest_bytes: proposed.len(),
			content_entries: document.project().contents.len(),
		},
		source: source_revision,
		requested_edits: edits.len(),
		difference: changed.then(|| source_difference(&source, proposed)),
	}
}

fn open_issue(error: ProjectEditError) -> Issue {
	match error {
		ProjectEditError::InvalidProject(error) => config_issue(error),
		ProjectEditError::Syntax { offset } => Issue::InvalidToml { offset },
		_ => Issue::InvalidManifest,
	}
}

impl From<ProjectEditError> for EditIssue {
	fn from(error: ProjectEditError) -> Self {
		match error {
			ProjectEditError::UnknownTarget { index } => Self::UnknownTarget { edit_index: index },
			ProjectEditError::ConflictingEdit { index } => {
				Self::ConflictingEdit { edit_index: index }
			}
			ProjectEditError::InvalidProject(error) => Self::InvalidResult {
				issue: config_issue(error),
			},
			_ => Self::EditFailed,
		}
	}
}

fn source_difference(source: &str, proposed: &str) -> SourceDifference {
	let start_byte = source
		.chars()
		.zip(proposed.chars())
		.take_while(|(before, after)| before == after)
		.map(|(ch, _)| ch.len_utf8())
		.sum::<usize>();
	let suffix_bytes = source[start_byte..]
		.chars()
		.rev()
		.zip(proposed[start_byte..].chars().rev())
		.take_while(|(before, after)| before == after)
		.map(|(ch, _)| ch.len_utf8())
		.sum::<usize>();
	SourceDifference {
		start_byte,
		replaced_bytes: source.len() - start_byte - suffix_bytes,
		replacement_bytes: proposed.len() - start_byte - suffix_bytes,
	}
}
