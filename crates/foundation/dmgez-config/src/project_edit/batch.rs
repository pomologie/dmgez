use serde::Deserialize;
use thiserror::Error;
use toml::{Table, Value};

use crate::{Coordinate, VirtualPath};

use super::{ContentChanges, ContentDraft, FieldChange, ProjectEdit};

/// Largest accepted edit batch document, measured in UTF-8 bytes.
pub const MAX_EDIT_BATCH_BYTES: usize = 1 << 20;
const MAX_OPERATIONS: usize = 4_096;
const SCHEMA_VERSION: i64 = 1;
const VERSION_KEY: &str = "schema_version";
const EDITS_KEY: &str = "edits";
const POSITION_FIELD: &str = "position";
const HIDDEN_FIELD: &str = "hidden";
const HIDE_EXTENSION_FIELD: &str = "hide_extension";

/// Errors while decoding a versioned project edit batch.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum EditBatchError {
	#[error("edit batch exceeds the {MAX_EDIT_BATCH_BYTES}-byte input limit")]
	TooLarge,
	#[error("invalid edit batch TOML (byte offset {offset:?})")]
	Syntax { offset: Option<usize> },
	#[error("edit batch requires an integer schema_version")]
	SchemaVersion,
	#[error("unsupported edit batch schema version {0}")]
	UnsupportedVersion(i64),
	#[error("edit batch requires an edits array and accepts no other fields")]
	Schema,
	#[error("edit batch exceeds the limit of {MAX_OPERATIONS} operations")]
	TooManyOperations,
	#[error("edit at zero-based index {index} does not match the edit batch schema")]
	Operation { index: usize },
	#[error("edit at zero-based index {index} both sets and clears {field}")]
	Conflict { index: usize, field: &'static str },
	#[error("modify edit at zero-based index {index} has no changes")]
	NoChanges { index: usize },
}

/// Decodes the ordered edits in a schema version 1 TOML batch.
///
/// Each entry in `edits` selects a snake_case `operation`. Image paths use
/// [`VirtualPath`] syntax. String settings retain their project syntax for
/// substitution and validation when the batch is applied. The batch leaves
/// the output destination to the rebuild caller.
///
/// # Errors
/// Returns errors for invalid TOML, an unsupported schema, malformed entries,
/// conflicting field instructions, or exceeded input limits.
pub fn parse_edit_batch(input: &str) -> Result<Vec<ProjectEdit>, EditBatchError> {
	if input.len() > MAX_EDIT_BATCH_BYTES {
		return Err(EditBatchError::TooLarge);
	}
	let mut document: Table = toml::from_str(input).map_err(|error| EditBatchError::Syntax {
		offset: error.span().map(|span| span.start),
	})?;
	let version = document
		.remove(VERSION_KEY)
		.and_then(|value| value.as_integer())
		.ok_or(EditBatchError::SchemaVersion)?;
	if version != SCHEMA_VERSION {
		return Err(EditBatchError::UnsupportedVersion(version));
	}
	let Some(Value::Array(edits)) = document.remove(EDITS_KEY) else {
		return Err(EditBatchError::Schema);
	};
	if !document.is_empty() {
		return Err(EditBatchError::Schema);
	}
	if edits.len() > MAX_OPERATIONS {
		return Err(EditBatchError::TooManyOperations);
	}
	edits
		.into_iter()
		.enumerate()
		.map(|(index, entry)| {
			let raw: RawEdit = entry
				.try_into()
				.map_err(|_| EditBatchError::Operation { index })?;
			raw.decode(index)
		})
		.collect()
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum RawEdit {
	AddCopy {
		source: String,
		destination: Option<VirtualPath>,
	},
	AddDirectory {
		destination: VirtualPath,
	},
	AddSymlink {
		destination: VirtualPath,
		target: String,
	},
	Modify {
		target: VirtualPath,
		destination: Option<VirtualPath>,
		position: Option<[Coordinate; 2]>,
		hidden: Option<bool>,
		hide_extension: Option<bool>,
		#[serde(default)]
		clear_position: bool,
		#[serde(default)]
		clear_hidden: bool,
		#[serde(default)]
		clear_hide_extension: bool,
	},
	Remove {
		target: VirtualPath,
	},
	SetImageName {
		name: String,
	},
}

impl RawEdit {
	fn decode(self, index: usize) -> Result<ProjectEdit, EditBatchError> {
		Ok(match self {
			Self::AddCopy {
				source,
				destination,
			} => ProjectEdit::Add(ContentDraft::Copy {
				source,
				destination,
			}),
			Self::AddDirectory { destination } => {
				ProjectEdit::Add(ContentDraft::Directory { destination })
			}
			Self::AddSymlink {
				destination,
				target,
			} => ProjectEdit::Add(ContentDraft::Symlink {
				destination,
				target,
			}),
			Self::Modify {
				target,
				destination,
				position,
				hidden,
				hide_extension,
				clear_position,
				clear_hidden,
				clear_hide_extension,
			} => {
				let changes = ContentChanges {
					destination,
					position: change(
						position.map(|point| point.map(Coordinate::get)),
						clear_position,
						index,
						POSITION_FIELD,
					)?,
					hidden: change(hidden, clear_hidden, index, HIDDEN_FIELD)?,
					hide_extension: change(
						hide_extension,
						clear_hide_extension,
						index,
						HIDE_EXTENSION_FIELD,
					)?,
				};
				if changes.destination.is_none()
					&& changes.position == FieldChange::Keep
					&& changes.hidden == FieldChange::Keep
					&& changes.hide_extension == FieldChange::Keep
				{
					return Err(EditBatchError::NoChanges { index });
				}
				ProjectEdit::Modify { target, changes }
			}
			Self::Remove { target } => ProjectEdit::Remove { target },
			Self::SetImageName { name } => ProjectEdit::SetImageName(name),
		})
	}
}

fn change<T>(
	value: Option<T>,
	clear: bool,
	index: usize,
	field: &'static str,
) -> Result<FieldChange<T>, EditBatchError> {
	match (value, clear) {
		(Some(_), true) => Err(EditBatchError::Conflict { index, field }),
		(Some(value), false) => Ok(FieldChange::Set(value)),
		(None, true) => Ok(FieldChange::Clear),
		(None, false) => Ok(FieldChange::Keep),
	}
}
