use std::num::NonZeroU32;

use serde::Serialize;

use crate::{DEFAULT_COMPARE_BYTES, bytes::decimal};

use super::{Options, xattrs::POLICY as XATTR_POLICY};

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
	/// Exact byte comparison with decoded records for matched regular Finder stores.
	#[cfg(feature = "finder-layout")]
	ExactBytesWithFinderLayout,
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
	ExtendedAttributes,
	FinderInfo,
	#[cfg(feature = "finder-layout")]
	FinderLayout,
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
			Self::ExtendedAttributes => "extended_attributes",
			Self::FinderInfo => "finder_info",
			#[cfg(feature = "finder-layout")]
			Self::FinderLayout => "finder_layout",
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
	QuarantineAssessment,
	FinderMetadata,
	FinderLayout,
	#[cfg(feature = "finder-layout")]
	PerObjectFinderInfo,
	NativeObjectIds,
	FilesystemAllocation,
	PhysicalContainerLayout,
	Signatures,
	RenameInference,
}

const COMPARED_FIELDS: [Field; 8] = [
	Field::Kind,
	Field::Mode,
	Field::Uid,
	Field::Gid,
	Field::HardLinkTopology,
	Field::DataFork,
	Field::ResourceFork,
	Field::SymlinkTarget,
];

const COMPARED_FIELDS_WITH_XATTRS: [Field; COMPARED_FIELDS.len() + 1] =
	with_field(&COMPARED_FIELDS, Field::ExtendedAttributes);

const UNCOMPARED_FIELDS: [Uncompared; 11] = [
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

const UNCOMPARED_FIELDS_WITH_XATTRS: [Uncompared; 10] = [
	Uncompared::Timestamps,
	Uncompared::AccessControlLists,
	Uncompared::FilesystemFlags,
	Uncompared::QuarantineAssessment,
	Uncompared::FinderMetadata,
	Uncompared::NativeObjectIds,
	Uncompared::FilesystemAllocation,
	Uncompared::PhysicalContainerLayout,
	Uncompared::Signatures,
	Uncompared::RenameInference,
];

const fn with_field<const N: usize>(base: &[Field], field: Field) -> [Field; N] {
	let mut fields = [field; N];
	fields.split_at_mut(base.len()).0.copy_from_slice(base);
	fields
}

const fn replace_finder_exclusion<const N: usize>(
	mut fields: [Uncompared; N],
	replacement: Uncompared,
) -> [Uncompared; N] {
	let mut index = 0;
	while index < fields.len() {
		if matches!(fields[index], Uncompared::FinderMetadata) {
			fields[index] = replacement;
		}
		index += 1;
	}
	fields
}

/// Scope and metadata limits for stored attribute comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct XattrPolicy {
	pub excluded_names: &'static [&'static str],
	pub max_attributes_per_object: NonZeroU32,
	pub max_attributes_per_side: usize,
	pub max_name_bytes_per_side: usize,
}

/// Scope declared by the semantic comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Policy {
	pub paths: PathMatching,
	pub contents: ContentComparison,
	pub compared: &'static [Field],
	pub excluded: &'static [Uncompared],
	#[serde(skip_serializing_if = "Option::is_none")]
	pub xattrs: Option<XattrPolicy>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub finder_info: Option<FinderInfoPolicy>,
	#[cfg(feature = "finder-layout")]
	#[serde(skip_serializing_if = "Option::is_none")]
	pub finder_layout: Option<FinderLayoutPolicy>,
}

impl Policy {
	pub(super) fn for_options(options: Options) -> Self {
		let policy = if options.xattrs {
			Self::with_xattrs()
		} else {
			Self::default()
		};
		#[cfg(feature = "finder-layout")]
		let policy = if options.finder_layout {
			policy.with_finder_layout()
		} else {
			policy
		};
		if options.finder_info {
			policy.with_finder_info()
		} else {
			policy
		}
	}

	fn with_xattrs() -> Self {
		Self {
			compared: &COMPARED_FIELDS_WITH_XATTRS,
			excluded: &UNCOMPARED_FIELDS_WITH_XATTRS,
			xattrs: Some(XATTR_POLICY),
			..Self::default()
		}
	}
}

impl Default for Policy {
	fn default() -> Self {
		Self {
			paths: PathMatching::ExactStoredText,
			contents: ContentComparison::ExactBytes,
			compared: &COMPARED_FIELDS,
			excluded: &UNCOMPARED_FIELDS,
			xattrs: None,
			finder_info: None,
			#[cfg(feature = "finder-layout")]
			finder_layout: None,
		}
	}
}

pub use finder_info::{FinderInfoComparison, FinderInfoPolicy};
mod finder_info;

#[cfg(feature = "finder-layout")]
pub use finder_layout::{FinderLayoutPolicy, FinderLayoutUncompared, FinderRecordChange};
#[cfg(feature = "finder-layout")]
mod finder_layout;

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

/// One attribute change at a common stored path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct XattrChange {
	pub name: String,
	pub kind: ChangeKind,
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
	/// Stored attribute changes in lexicographic name order.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub xattrs: Vec<XattrChange>,
	/// Finder record identity changes in name and field order.
	#[cfg(feature = "finder-layout")]
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub finder_layout: Vec<FinderRecordChange>,
}

impl Change {
	pub(super) fn new(path: &[String], kind: ChangeKind, fields: Vec<Field>) -> Self {
		Self {
			path: path.to_vec(),
			kind,
			fields,
			xattrs: Vec::new(),
			#[cfg(feature = "finder-layout")]
			finder_layout: Vec::new(),
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
	/// Paired fork and attribute chunks counted once. Whole link targets,
	/// Finder stores, and FinderInfo are charged the larger of the two input lengths.
	#[serde(serialize_with = "decimal")]
	pub compared_bytes: u64,
	/// Differences in lexicographic component order.
	pub changes: Vec<Change>,
}
