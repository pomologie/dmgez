use serde::Serialize;

const SCHEMA_VERSION: u32 = 1;

/// The file content that will be projected to the destination.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum SelectionReport {
	DataFork,
	ResourceFork,
	DataTree,
}

/// Filesystem information outside the selected projection, if present on the source.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Omission {
	OtherForks,
	Ownership,
	Permissions,
	Timestamps,
	ExtendedAttributes,
	FinderMetadata,
	HardLinkTopology,
}

const OMITTED_METADATA: &[Omission] = &[
	Omission::OtherForks,
	Omission::Ownership,
	Omission::Permissions,
	Omission::Timestamps,
	Omission::ExtendedAttributes,
	Omission::FinderMetadata,
	Omission::HardLinkTopology,
];

/// Content, effects, and metadata scope of an admitted extraction.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct PlanReport {
	pub schema_version: u32,
	pub selection: SelectionReport,
	pub files: usize,
	/// Includes the new output directory when extracting a tree.
	pub directories: usize,
	pub bytes: u64,
	/// Number of selected file paths with shared source storage that become independent files.
	pub hard_links_materialized: usize,
	/// Categories outside this projection, even when presence was not inspected.
	pub omitted: &'static [Omission],
}

impl PlanReport {
	pub(super) const fn new(selection: SelectionReport) -> Self {
		Self {
			schema_version: SCHEMA_VERSION,
			selection,
			files: 0,
			directories: 0,
			bytes: 0,
			hard_links_materialized: 0,
			omitted: OMITTED_METADATA,
		}
	}
}

/// Confirmed publications. On failure the destination may contain these outputs.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Progress {
	pub directories_created: usize,
	pub files_published: usize,
	pub bytes_published: u64,
	/// A directory was created but its attachment could not be confirmed.
	pub directory_creation_unconfirmed: bool,
}

impl Progress {
	/// Whether any destination entry was published or may have been created.
	#[must_use]
	pub const fn has_destination_effects(self) -> bool {
		self.directories_created != 0
			|| self.files_published != 0
			|| self.directory_creation_unconfirmed
	}
}
