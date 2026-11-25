use serde::Serialize;

use crate::{DEFAULT_COMPARE_BYTES, bytes::decimal};

const DEFAULT_MAX_ENTRIES: usize = 16_384;
const DEFAULT_MAX_DEPTH: usize = 128;
const DEFAULT_MAX_PATH_BYTES: usize = 4 * 1024 * 1024;

/// Work limits for a semantic comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Limits {
	/// Maximum number of admitted paths in each catalog, including its root.
	pub max_entries: usize,
	/// Maximum number of components below each volume root.
	pub max_depth: usize,
	/// Maximum aggregate stored absolute-path bytes in each catalog.
	pub max_path_bytes: usize,
	/// Maximum logical comparison payload bytes per side.
	#[serde(serialize_with = "decimal")]
	pub max_bytes: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_entries: DEFAULT_MAX_ENTRIES,
			max_depth: DEFAULT_MAX_DEPTH,
			max_path_bytes: DEFAULT_MAX_PATH_BYTES,
			max_bytes: DEFAULT_COMPARE_BYTES,
		}
	}
}

/// Result for the fields covered by the reported comparison policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Equal,
	Different,
}

impl Outcome {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Equal => "equal",
			Self::Different => "different",
		}
	}
}

/// How stored names are paired between two trees.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PathMatching {
	ExactStoredText,
}

/// How comparable logical payloads are compared.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ContentComparison {
	ExactBytes,
}

/// A supported field that changed within one common path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Field {
	Kind,
	Mode,
	Uid,
	Gid,
	HardLinkTopology,
	DataFork,
	ResourceFork,
	SymlinkTarget,
}

impl Field {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Kind => "kind",
			Self::Mode => "mode",
			Self::Uid => "uid",
			Self::Gid => "gid",
			Self::HardLinkTopology => "hard_link_topology",
			Self::DataFork => "data_fork",
			Self::ResourceFork => "resource_fork",
			Self::SymlinkTarget => "symlink_target",
		}
	}
}

/// Information outside the exact logical comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Uncompared {
	Timestamps,
	ExtendedAttributesExceptResourceFork,
	AccessControlLists,
	FilesystemFlags,
	Quarantine,
	FinderMetadata,
	NativeObjectIds,
	FilesystemAllocation,
	PhysicalContainerLayout,
	Signatures,
	RenameInference,
}

const COMPARED_FIELDS: &[Field] = &[
	Field::Kind,
	Field::Mode,
	Field::Uid,
	Field::Gid,
	Field::HardLinkTopology,
	Field::DataFork,
	Field::ResourceFork,
	Field::SymlinkTarget,
];

const UNCOMPARED_FIELDS: &[Uncompared] = &[
	Uncompared::Timestamps,
	Uncompared::ExtendedAttributesExceptResourceFork,
	Uncompared::AccessControlLists,
	Uncompared::FilesystemFlags,
	Uncompared::Quarantine,
	Uncompared::FinderMetadata,
	Uncompared::NativeObjectIds,
	Uncompared::FilesystemAllocation,
	Uncompared::PhysicalContainerLayout,
	Uncompared::Signatures,
	Uncompared::RenameInference,
];

/// Scope declared by the semantic comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Policy {
	pub paths: PathMatching,
	pub contents: ContentComparison,
	pub compared: &'static [Field],
	pub excluded: &'static [Uncompared],
}

impl Default for Policy {
	fn default() -> Self {
		Self {
			paths: PathMatching::ExactStoredText,
			contents: ContentComparison::ExactBytes,
			compared: COMPARED_FIELDS,
			excluded: UNCOMPARED_FIELDS,
		}
	}
}

/// Type of difference at a stored path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
	Added,
	Removed,
	Changed,
}

impl ChangeKind {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Added => "added",
			Self::Removed => "removed",
			Self::Changed => "changed",
		}
	}
}

/// A difference at one exact stored path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Change {
	/// Literal stored components from the volume root. The root is empty.
	pub path: Vec<String>,
	pub kind: ChangeKind,
	/// Changed fields of a path common to both volumes.
	pub fields: Vec<Field>,
}

impl Change {
	pub(super) fn new(path: &[String], kind: ChangeKind, fields: Vec<Field>) -> Self {
		Self {
			path: path.to_vec(),
			kind,
			fields,
		}
	}
}

/// Complete result for the declared, bounded semantic comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Report {
	pub schema_version: u32,
	pub outcome: Outcome,
	pub policy: Policy,
	pub limits: Limits,
	/// Number of paths scanned, including the root.
	pub left_entries: usize,
	/// Number of paths scanned, including the root.
	pub right_entries: usize,
	/// Paired fork chunks counted once, with the larger length charged for links.
	#[serde(serialize_with = "decimal")]
	pub compared_bytes: u64,
	/// Differences in lexicographic component order.
	pub changes: Vec<Change>,
}
