//! Validated, comment-preserving edits to a project document in memory.
//!
//! A caller can preview or build from the typed project, compare document text
//! before saving, and retain the revision ID associated with a completed save.

use std::collections::{BTreeMap, VecDeque};

use thiserror::Error;
use toml_edit::DocumentMut;

use crate::{ConfigError, MAX_MANIFEST_BYTES, Project, VirtualPath};

mod batch;
mod patch;

pub use batch::{EditBatchError, MAX_EDIT_BATCH_BYTES, parse_edit_batch};

const MAX_HISTORY_BYTES: usize = MAX_MANIFEST_BYTES * 2;
const MAX_HISTORY_REVISIONS: usize = 64;

/// A revision identifier local to one open document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RevisionId(u64);

/// A project with its preserved syntax, validated settings, and edit history.
pub struct ProjectDocument {
	project: Project,
	syntax: DocumentMut,
	defines: BTreeMap<String, String>,
	history: VecDeque<Snapshot>,
	history_bytes: usize,
	cursor: usize,
	last_id: RevisionId,
}

struct Snapshot {
	id: RevisionId,
	source: String,
}

/// A user operation on the project source.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ProjectEdit {
	/// Append a content declaration.
	Add(ContentDraft),
	/// Modify a content declaration selected by its destination before the batch.
	Modify {
		target: VirtualPath,
		changes: ContentChanges,
	},
	/// Remove a declaration selected by its destination before the batch.
	Remove { target: VirtualPath },
	/// Set the image label using the project's ordinary string syntax.
	SetImageName(String),
	/// Set the output path using the project's ordinary string syntax.
	SetOutputPath(String),
}

/// An entry to append to the project. Strings use the project's ordinary syntax.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ContentDraft {
	Copy {
		source: String,
		destination: Option<VirtualPath>,
	},
	Directory {
		destination: VirtualPath,
	},
	Symlink {
		destination: VirtualPath,
		target: String,
	},
}

/// Fields to change on one declaration. A new destination changes that
/// declaration; changes to other declarations can join the same batch.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ContentChanges {
	pub destination: Option<VirtualPath>,
	pub position: FieldChange<[f64; 2]>,
	pub hidden: FieldChange<bool>,
	pub hide_extension: FieldChange<bool>,
}

/// A change to an optional setting in the source document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldChange<T> {
	#[default]
	Keep,
	Set(T),
	Clear,
}

/// A project could not be loaded, changed, or navigated.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ProjectEditError {
	#[error(transparent)]
	InvalidProject(#[from] ConfigError),
	#[error("project syntax could not be preserved (byte offset {offset:?})")]
	Syntax { offset: Option<usize> },
	#[error("edit {index} refers to a destination absent before the batch")]
	UnknownTarget { index: usize },
	#[error("edit {index} modifies a declaration also removed by the batch")]
	ConflictingEdit { index: usize },
	#[error("project syntax does not match the validated project structure")]
	Structure,
	#[error("project revision identifiers are exhausted")]
	RevisionLimit,
}

impl ProjectDocument {
	/// Loads a project document with its original text and parsed settings.
	/// # Errors
	/// Rejects an unsupported schema, invalid project, or unpreservable syntax.
	pub fn open(input: &str) -> Result<Self, ProjectEditError> {
		Self::open_with_defines(input, BTreeMap::new())
	}

	/// Loads a document using caller-supplied defines throughout this session.
	/// # Errors
	/// Rejects invalid project settings, syntax, and unresolved definitions.
	pub fn open_with_defines(
		input: &str,
		defines: BTreeMap<String, String>,
	) -> Result<Self, ProjectEditError> {
		let (project, syntax) = parse_document(input, &defines)?;
		let id = RevisionId(0);
		Ok(Self {
			project,
			syntax,
			defines,
			history: VecDeque::from([Snapshot {
				id,
				source: input.to_owned(),
			}]),
			history_bytes: input.len(),
			cursor: 0,
			last_id: id,
		})
	}

	/// Returns the semantic project for the current revision.
	#[must_use]
	pub const fn project(&self) -> &Project {
		&self.project
	}

	/// Returns the current source, including comments and spacing.
	#[must_use]
	pub fn source(&self) -> &str {
		&self.history[self.cursor].source
	}

	/// Returns the current revision. Undo and redo restore earlier IDs.
	#[must_use]
	pub fn revision(&self) -> RevisionId {
		self.history[self.cursor].id
	}

	/// Applies one gesture or command group atomically and returns `true` if
	/// the text changed. Targets refer to the declarations at batch start, so
	/// a group can swap destinations and move selected items together.
	///
	/// A successful new edit replaces the redo branch. History retains the
	/// most recent revisions within the editor's source byte and count bounds.
	/// # Errors
	/// Returns an error for missing or incompatible targets, an invalid final
	/// project, or a revision limit. The current state and history stay intact.
	pub fn apply(&mut self, edits: &[ProjectEdit]) -> Result<bool, ProjectEditError> {
		if edits.is_empty() {
			return Ok(false);
		}
		let mut syntax = self.syntax.clone();
		patch::apply(&mut syntax, &self.project, edits)?;
		let mut source = syntax.to_string();
		if !self.source().ends_with('\n') && source.ends_with('\n') {
			source.pop();
		}
		if self.source().contains("\r\n") && has_uniform_crlf(self.source()) {
			source = source.replace('\n', "\r\n");
		}
		if source == self.source() {
			return Ok(false);
		}
		let project = Project::parse_with_defines(&source, &self.defines)?;
		let next = self
			.last_id
			.0
			.checked_add(1)
			.ok_or(ProjectEditError::RevisionLimit)?;
		let id = RevisionId(next);
		self.commit(Snapshot { id, source }, project, syntax);
		Ok(true)
	}

	/// Restores the previous retained revision. Returns `false` at the start
	/// of retained history.
	/// # Errors
	/// Returns a validation or syntax error if the earlier revision cannot be loaded.
	pub fn undo(&mut self) -> Result<bool, ProjectEditError> {
		match self.cursor.checked_sub(1) {
			Some(index) => self.select(index),
			None => Ok(false),
		}
	}

	/// Restores the next retained revision. Returns `false` at the current tip.
	/// # Errors
	/// Returns a validation or syntax error if the later revision cannot be loaded.
	pub fn redo(&mut self) -> Result<bool, ProjectEditError> {
		self.select(self.cursor + 1)
	}

	fn select(&mut self, index: usize) -> Result<bool, ProjectEditError> {
		let Some(snapshot) = self.history.get(index) else {
			return Ok(false);
		};
		let (project, syntax) = parse_document(&snapshot.source, &self.defines)?;
		self.project = project;
		self.syntax = syntax;
		self.cursor = index;
		Ok(true)
	}

	fn commit(&mut self, snapshot: Snapshot, project: Project, syntax: DocumentMut) {
		for discarded in self.history.drain(self.cursor + 1..) {
			self.history_bytes -= discarded.source.len();
		}
		self.history_bytes += snapshot.source.len();
		self.last_id = snapshot.id;
		self.history.push_back(snapshot);
		while self.history_bytes > MAX_HISTORY_BYTES || self.history.len() > MAX_HISTORY_REVISIONS {
			let oldest = self
				.history
				.pop_front()
				.expect("the current revision fits in history");
			self.history_bytes -= oldest.source.len();
		}
		self.cursor = self.history.len() - 1;
		self.project = project;
		self.syntax = syntax;
	}
}

fn parse_document(
	source: &str,
	defines: &BTreeMap<String, String>,
) -> Result<(Project, DocumentMut), ProjectEditError> {
	let project = Project::parse_with_defines(source, defines)?;
	let syntax = source
		.parse::<DocumentMut>()
		.map_err(|error| ProjectEditError::Syntax {
			offset: error.span().map(|span| span.start),
		})?;
	Ok((project, syntax))
}

fn has_uniform_crlf(source: &str) -> bool {
	source
		.match_indices('\n')
		.all(|(offset, _)| source[..offset].ends_with('\r'))
}
