use std::fmt;

use dmgez_vfs::volume::{NodeKind, RawName};
use serde::Serialize;

use crate::{FilesystemObjectId, ReportOperation, SelectedWindowId};

/// Stored name bytes or code units, with no normalization or replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum Name {
	Utf8(String),
	Utf16(Vec<u16>),
}

impl Name {
	pub(super) fn from_native(name: RawName) -> Option<Self> {
		match name {
			RawName::Utf8(name) => Some(Self::Utf8(name)),
			RawName::Utf16(name) => Some(Self::Utf16(name)),
			_ => None,
		}
	}
}

impl fmt::Display for Name {
	/// Renders display text with control characters and unpaired units escaped.
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Utf8(name) => write!(formatter, "{}", name.escape_debug()),
			Self::Utf16(name) => {
				for unit in char::decode_utf16(name.iter().copied()) {
					match unit {
						Ok(character) => write!(formatter, "{}", character.escape_debug())?,
						Err(error) => write!(formatter, "\\u{{{:x}}}", error.unpaired_surrogate())?,
					}
				}
				Ok(())
			}
		}
	}
}

/// Visible object kind in a volume-query report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Kind {
	Directory,
	RegularFile,
	Symlink,
	Other,
}

impl Kind {
	/// Stable spelling used in machine-readable reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Directory => "directory",
			Self::RegularFile => "regular_file",
			Self::Symlink => "symlink",
			Self::Other => "other",
		}
	}
}

impl From<NodeKind> for Kind {
	fn from(kind: NodeKind) -> Self {
		match kind {
			NodeKind::Directory => Self::Directory,
			NodeKind::RegularFile => Self::RegularFile,
			NodeKind::Symlink => Self::Symlink,
			_ => Self::Other,
		}
	}
}

/// A direct child in the filesystem's native directory order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct EntryReport {
	pub name: Name,
	pub kind: Kind,
}

/// A bounded directory page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ListReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	pub volume: SelectedWindowId,
	/// The absolute volume path specified by the caller.
	pub path: String,
	pub entries: Vec<EntryReport>,
	/// A zero-based offset for the next page in the same immutable view.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub next_offset: Option<u64>,
	/// More entries exist past the supported pagination offset.
	pub truncated: bool,
}

/// One visible descendant of the starting directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct TreeEntryReport {
	/// The number of directory edges from the starting directory.
	pub depth: u16,
	pub name: Name,
	pub kind: Kind,
}

/// An ordered tree within the requested depth and entry bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct TreeReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	pub volume: SelectedWindowId,
	/// The absolute volume path specified by the caller.
	pub path: String,
	pub entries: Vec<TreeEntryReport>,
	/// The entry ceiling omitted further objects inside the requested depth.
	pub truncated: bool,
}

/// Catalog metadata for an object and its link target, when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct StatReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	pub volume: SelectedWindowId,
	/// The absolute volume path specified by the caller.
	pub path: String,
	/// Native spelling of the terminal component. The view root has no name here.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<Name>,
	pub kind: Kind,
	pub mode: u16,
	pub uid: u32,
	pub gid: u32,
	/// The numeric engine object ID within this view, serialized as a decimal string.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub native_id: Option<FilesystemObjectId>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub hard_link_count: Option<u32>,
	/// The engine storage identity of a hard-link group, scoped to this view.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub hard_link_storage_id: Option<FilesystemObjectId>,
	/// The stored symlink payload bytes.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub link_target: Option<Vec<u8>>,
}
