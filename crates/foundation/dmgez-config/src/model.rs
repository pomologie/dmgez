use std::num::NonZeroU64;

use serde::Deserialize;
use thiserror::Error;

use crate::{
	AutoFlag, Bytes, LocalPath, Size, Text,
	content::{Content, RawContent},
	finder::{Finder, RawVolumeIcon, VolumeIcon},
	release::{License, Notarization, Release, Signing},
};

const SECTOR_BYTES: u64 = 512;
const ZLIB_MAX_CHUNK_BYTES: u64 = 1 << 20;
const ZLIB_MAX_LEVEL: u8 = 9;
const HFS_MAX_NAME_UNITS: usize = 255;
const VOLUME_NAME_SEPARATORS: [char; 2] = ['/', ':'];

/// A validated version-one project. Recognized draft options are retained so a
/// caller can report capabilities before accessing any host paths.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Project {
	pub image: Image,
	pub contents: Vec<Content>,
	pub compression: Compression,
	pub performance: Performance,
	pub metadata: Metadata,
	pub build: Build,
	pub finder: Option<Finder>,
	pub volume_icon: Option<VolumeIcon>,
	pub signing: Option<Signing>,
	pub notarization: Option<Notarization>,
	pub release: Option<Release>,
	pub license: Option<License>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProject {
	pub image: Image,
	#[serde(default)]
	pub contents: Vec<RawContent>,
	#[serde(default)]
	pub compression: Compression,
	#[serde(default)]
	pub performance: Performance,
	#[serde(default)]
	pub metadata: Metadata,
	#[serde(default)]
	pub build: Build,
	pub finder: Option<Finder>,
	pub volume_icon: Option<RawVolumeIcon>,
	pub signing: Option<Signing>,
	pub notarization: Option<Notarization>,
	pub release: Option<Release>,
	pub license: Option<License>,
}

/// Image settings. Capacity is validated before the project is returned.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Image {
	pub name: Text,
	pub output: LocalPath,
	#[serde(default)]
	pub format: ImageFormat,
	#[serde(default)]
	pub filesystem: Filesystem,
	#[serde(default)]
	size: Size,
	#[serde(default)]
	shrink: bool,
	#[serde(default)]
	pub free_space: Bytes,
	#[serde(default)]
	pub partition: Partition,
}

impl Image {
	/// Returns the capacity policy after resolving size and shrink together.
	#[must_use]
	pub const fn capacity(&self) -> Capacity {
		match self.size {
			Size::Auto => Capacity::Auto {
				shrink: self.shrink,
			},
			Size::Fixed(bytes) if self.shrink => Capacity::ShrinkWithin(bytes),
			Size::Fixed(bytes) => Capacity::Exact(bytes),
		}
	}

	pub(crate) fn validate(&self) -> Result<(), ImageError> {
		if self.name.as_str().contains(VOLUME_NAME_SEPARATORS) {
			return Err(ImageError::NameSeparator);
		}
		if matches!(self.filesystem, Filesystem::Hfsplus | Filesystem::Hfsx)
			&& self.name.as_str().encode_utf16().count() > HFS_MAX_NAME_UNITS
		{
			return Err(ImageError::NameLength);
		}
		if let Size::Fixed(bytes) = self.size {
			if !positive_sectors(bytes) {
				return Err(ImageError::SizeAlignment);
			}
			if self.free_space > bytes {
				return Err(ImageError::FreeSpaceExceedsSize);
			}
		}
		Ok(())
	}
}

/// Capacity after applying the shrink policy, before planning actual payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Capacity {
	/// Compute capacity from the plan. Shrinking requests its minimum capacity.
	Auto { shrink: bool },
	/// Keep the specified capacity.
	Exact(Bytes),
	/// Find a minimum capacity no greater than the specified limit.
	ShrinkWithin(Bytes),
}

/// Invalid image settings.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ImageError {
	#[error("image name must not contain a slash or colon")]
	NameSeparator,
	#[error("HFS volume name exceeds {HFS_MAX_NAME_UNITS} UTF-16 code units")]
	NameLength,
	#[error("explicit image size must be positive and sector-aligned")]
	SizeAlignment,
	#[error("minimum free space exceeds the explicit image size")]
	FreeSpaceExceedsSize,
}

/// Recognized image format identifiers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
	Udro,
	#[default]
	Udzo,
	Ulfo,
	Ulmo,
	Raw,
	Udrw,
	Udsb,
	Sparseimage,
	Asif,
	Udbz,
	Udco,
}

/// Recognized volume filesystem identifiers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Filesystem {
	#[default]
	Hfsplus,
	Hfsx,
	Apfs,
}

/// Recognized partition layouts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Partition {
	#[default]
	None,
	Gpt,
	Apm,
	Mbr,
}

/// User-supplied compression settings.
#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Compression {
	pub level: Option<u8>,
	pub block_size: Option<Bytes>,
}

impl Compression {
	pub(crate) fn validate(&self, format: ImageFormat) -> Result<(), CompressionError> {
		if (self.level.is_some() || self.block_size.is_some()) && format != ImageFormat::Udzo {
			return Err(CompressionError::Format);
		}
		if self.level.is_some_and(|level| level > ZLIB_MAX_LEVEL) {
			return Err(CompressionError::Level);
		}
		if self
			.block_size
			.is_some_and(|bytes| !positive_sectors(bytes) || bytes.get() > ZLIB_MAX_CHUNK_BYTES)
		{
			return Err(CompressionError::BlockSize);
		}
		Ok(())
	}
}

fn positive_sectors(bytes: Bytes) -> bool {
	bytes.get() != 0 && bytes.get().is_multiple_of(SECTOR_BYTES)
}

/// Invalid compression settings.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum CompressionError {
	#[error("compression options require the UDZO format")]
	Format,
	#[error("compression level exceeds the zlib range")]
	Level,
	#[error("compression block size must be sector-aligned and within the zlib limit")]
	BlockSize,
}

/// Resource budgets requested by the project. Zero threads requests automatic sizing.
#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
#[serde(default, deny_unknown_fields)]
pub struct Performance {
	pub parallel: AutoFlag,
	pub threads: u64,
	pub memory_limit: Option<Bytes>,
	pub max_entries: Option<NonZeroU64>,
	pub max_metadata_bytes: Option<Bytes>,
	pub max_runs: Option<NonZeroU64>,
}

impl Performance {
	pub(crate) fn validate(&self) -> Result<(), PerformanceError> {
		if self.memory_limit.is_some_and(|limit| limit.get() == 0) {
			return Err(PerformanceError::MemoryLimit);
		}
		if self
			.max_metadata_bytes
			.is_some_and(|limit| limit.get() == 0)
		{
			return Err(PerformanceError::MetadataLimit);
		}
		Ok(())
	}
}

/// Invalid resource budgets.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum PerformanceError {
	#[error("memory limit must be positive")]
	MemoryLimit,
	#[error("metadata byte limit must be positive")]
	MetadataLimit,
}

/// How representational metadata losses should be handled.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum MetadataPolicy {
	#[default]
	Strict,
	Report,
}

/// Per-kind metadata preservation requests.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(default, deny_unknown_fields)]
pub struct Metadata {
	pub policy: MetadataPolicy,
	pub sidecar: Option<LocalPath>,
	pub preserve_permissions: bool,
	pub preserve_ownership: bool,
	pub preserve_timestamps: bool,
	pub preserve_extended_attributes: bool,
	pub preserve_resource_forks: bool,
	pub preserve_finder_info: bool,
	pub preserve_hard_links: bool,
}

impl Default for Metadata {
	fn default() -> Self {
		Self {
			policy: MetadataPolicy::Strict,
			sidecar: None,
			preserve_permissions: true,
			preserve_ownership: true,
			preserve_timestamps: true,
			preserve_extended_attributes: true,
			preserve_resource_forks: true,
			preserve_finder_info: true,
			preserve_hard_links: true,
		}
	}
}

/// Execution policy. The parser itself performs no build operations.
#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
#[serde(default, deny_unknown_fields)]
pub struct Build {
	pub reproducible: bool,
	pub source_date_epoch: Option<u64>,
	pub verify_output: bool,
	pub overwrite: bool,
	pub fail_on_metadata_loss: Option<bool>,
}

impl Build {
	/// Resolves the default metadata loss policy from the selected profile.
	#[must_use]
	pub fn fail_on_metadata_loss(&self, metadata: &Metadata) -> bool {
		self.fail_on_metadata_loss
			.unwrap_or(metadata.policy == MetadataPolicy::Strict)
	}
}
