//! Bounded allocation and catalog-size analysis for an admitted HFS+ or HFSX volume.

use std::{collections::TryReserveError, fmt};

use dmgez_vfs::{Disk, LogicalDisk, VolumeId};
use hfsplus_format::volume::{CatalogId, Signature};
use hfsplus_reader::{
	allocation::{AllocationError, AllocationLimits, AllocationReport},
	catalog::{CatalogError, HardLinkError},
	decmpfs::LogicalDataError,
};
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::browse::{self, SourceError};

mod walk;

const DEFAULT_MAX_ENTRIES: u64 = 100_000;
const DEFAULT_LARGEST_FILES: usize = 20;
const MAX_DEPTH: u16 = 64;
const PATH_SEPARATOR: char = '/';

/// Largest number of ranked file paths a report can retain.
pub const MAX_LARGEST_FILES: usize = 256;

/// Independent bounds for volume reads, allocation checks, and the catalog walk.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Limits {
	pub source: browse::Limits,
	pub allocation: AllocationLimits,
	/// Maximum number of visible entries counted below the volume root.
	pub max_entries: u64,
	/// Maximum number of component edges from the root, from 1 to 64.
	pub max_depth: u16,
	/// Maximum number of ranked paths retained, from 1 to 256.
	pub largest_files: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			source: browse::Limits::default(),
			allocation: AllocationLimits::default(),
			max_entries: DEFAULT_MAX_ENTRIES,
			max_depth: MAX_DEPTH,
			largest_files: DEFAULT_LARGEST_FILES,
		}
	}
}

/// Admitted volume signature family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
	HfsPlus,
	Hfsx,
}

impl Format {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::HfsPlus => "hfsplus",
			Self::Hfsx => "hfsx",
		}
	}
}

impl From<Signature> for Format {
	fn from(signature: Signature) -> Self {
		match signature {
			Signature::HfsPlus => Self::HfsPlus,
			Signature::HfsX => Self::Hfsx,
		}
	}
}

/// Counts from the allocation bitmap checked against the volume's header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Allocation {
	/// Bytes in one volume allocation block.
	pub block_size: u32,
	pub total_blocks: u32,
	pub free_blocks: u32,
	/// The two header regions and allocated extents of resolved special files.
	pub metadata_blocks: u32,
	/// Remaining set bits. These blocks have no ownership claim in this check.
	pub other_allocated_blocks: u32,
}

impl Allocation {
	fn checked(report: AllocationReport, block_size: u32) -> Self {
		Self {
			block_size,
			total_blocks: report.total_blocks,
			free_blocks: report.free_blocks,
			metadata_blocks: report.metadata_blocks,
			other_allocated_blocks: report.other_allocated_blocks,
		}
	}
}

/// Work ceilings that excluded further visible descendants from the analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
	Entries,
	Depth,
}

impl Limit {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Entries => "entries",
			Self::Depth => "depth",
		}
	}
}

/// Reason a visible file record could not contribute a known logical size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeUnavailable {
	CompressionDisabled,
	UnsupportedCompression,
	CompressionLimit,
	DirectoryLink,
}

impl SizeUnavailable {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::CompressionDisabled => "compression_disabled",
			Self::UnsupportedCompression => "unsupported_compression",
			Self::CompressionLimit => "compression_limit",
			Self::DirectoryLink => "directory_link",
		}
	}
}

/// Number of entries whose sizes were unavailable for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct UnavailableSize {
	pub reason: SizeUnavailable,
	pub files: u64,
}

/// A visible regular file with an admitted logical data length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct File {
	/// Exact UTF-16 components in the visible volume path.
	pub path: Vec<Vec<u16>>,
	/// Visible object identity within this catalog.
	pub catalog_id: u32,
	/// Identity that owns the content, shared by hard-link aliases.
	pub inode_id: u32,
	#[serde(serialize_with = "decimal")]
	pub logical_bytes: u64,
}

impl File {
	/// Escapes controls, literal separators, and unpaired UTF-16 units for text output.
	#[must_use]
	pub fn display_path(&self) -> impl fmt::Display + '_ {
		DisplayPath(&self.path)
	}
}

struct DisplayPath<'names>(&'names [Vec<u16>]);

impl fmt::Display for DisplayPath<'_> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		use fmt::Write;

		for name in self.0 {
			formatter.write_char(PATH_SEPARATOR)?;
			for unit in char::decode_utf16(name.iter().copied()) {
				match unit {
					Ok(PATH_SEPARATOR) => write!(formatter, "\\{PATH_SEPARATOR}")?,
					Ok(ch) => write!(formatter, "{}", ch.escape_debug())?,
					Err(error) => write!(formatter, "\\u{{{:x}}}", error.unpaired_surrogate())?,
				}
			}
		}
		Ok(())
	}
}

/// Visible namespace observations. Hard-link paths count individually.
/// Internal catalog records are excluded. Sizes use the logical data fork
/// only and do not measure resource forks, attributes, or physical allocation.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Content {
	pub entries: u64,
	pub directories: u64,
	pub regular_files: u64,
	pub symlinks: u64,
	pub other_entries: u64,
	pub sized_files: u64,
	/// The walk ended within its bounds and every regular file had a known size.
	pub complete: bool,
	pub unavailable_sizes: Vec<UnavailableSize>,
	pub limits_reached: Vec<Limit>,
	/// Ranked by descending logical bytes, then ascending visible catalog ID.
	pub largest_files: Vec<File>,
}

/// HFS-family volume observations. This does not verify stored payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Report {
	pub format: Format,
	pub allocation: Allocation,
	pub content: Content,
}

/// Volume admission, allocation check, or bounded traversal failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("could not admit the selected HFS-family volume")]
	Admission(#[from] browse::Error),
	#[error("could not check HFS-family allocation")]
	Allocation(#[from] AllocationError<SourceError>),
	#[error("could not walk the HFS-family catalog")]
	Catalog(#[from] CatalogError<SourceError>),
	#[error("could not resolve a visible HFS-family file")]
	HardLink(#[from] HardLinkError<SourceError>),
	#[error("could not open the logical data view of an HFS-family file")]
	LogicalData(#[from] LogicalDataError<SourceError>),
	#[error("visible HFS-family file {} could not be resolved", .0.0)]
	MissingFile(CatalogId),
	#[error("could not reserve bounded HFS-family analysis data")]
	Memory(#[from] TryReserveError),
	#[error("HFS-family analysis depth must be from 1 to {MAX_DEPTH}")]
	Depth,
	#[error("ranked HFS-family file limit must be from 1 to {MAX_LARGEST_FILES}")]
	LargestFiles,
}

/// Checks allocation and summarizes visible file sizes in one budgeted session.
///
/// The scan opens logical forks only to obtain their lengths. Qualified inline
/// compression can decode during the open. Other file payload bytes are not
/// read by this analysis.
///
/// # Errors
/// Returns typed errors for invalid limits, unsupported or corrupt allocation
/// data, and read or catalog failures. Traversal ceilings and unsupported
/// compression produce explicit incomplete content coverage in a valid report.
pub fn analyze<S: LogicalDisk>(
	disk: &mut Disk<S>,
	selected: VolumeId,
	limits: Limits,
) -> Result<Report, Error> {
	if !(1..=MAX_DEPTH).contains(&limits.max_depth) {
		return Err(Error::Depth);
	}
	if !(1..=MAX_LARGEST_FILES).contains(&limits.largest_files) {
		return Err(Error::LargestFiles);
	}
	browse::with_catalog(disk, selected, limits.source, |volume, catalog| {
		let header = volume.header();
		let checked = volume.check_metadata_allocation(limits.allocation)?;
		Ok(Report {
			format: header.signature.into(),
			allocation: Allocation::checked(checked, header.geometry.block_size()),
			content: walk::summarize(&catalog, limits)?,
		})
	})?
}

fn decimal<S: Serializer>(bytes: &u64, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.collect_str(bytes)
}
