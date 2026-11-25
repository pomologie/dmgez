use thiserror::Error;

use super::PreflightError;

/// Whether the service composer can execute a build capability.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Availability {
	/// The capability is excluded from this application build.
	#[default]
	NotCompiled,
	/// The capability is compiled but cannot run in this environment.
	Unavailable,
	/// A qualified implementation is available for the operation.
	Available,
}

/// Backends that compose with the image builder.
///
/// Callers report availability after testing the configured backend and profile.
/// Codec and scheduler features are determined from this crate's own Cargo
/// configuration and checked again by the streaming writer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct BuildCapabilities {
	/// Clean HFS+ construction with the printable ASCII catalog profile.
	pub hfsplus_ascii: Availability,
	/// Clean HFS+ construction with historical Unicode name admission and collation.
	pub hfsplus_unicode: Availability,
	/// Finder metadata store writing for icon and list layouts.
	pub finder_layout: Availability,
	/// Binding a captured Finder background to frozen filesystem catalog identities.
	pub finder_image_background: Availability,
	/// Preparation of the volume icon asset and its root catalog custom-icon flag.
	pub finder_volume_icon: Availability,
	/// Preparation of per-entry Finder icons and their filesystem metadata.
	pub finder_custom_icon: Availability,
	/// Application of explicit hidden and hide-extension filesystem flags.
	pub finder_flags: Availability,
	/// Merge a declared AppleDouble file with native source metadata before sealing.
	pub metadata_sidecar: Availability,
	/// Independent container and filesystem verification before publication.
	pub verify: Availability,
	/// Staged replacement with source-identity and alias protection.
	pub atomic_replace: Availability,
	/// Release stages provided by the higher-level application.
	pub release: Availability,
}

/// A capability that can prevent admission.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum BuildFeature {
	#[error("HFS+ construction")]
	Hfsplus,
	#[error("zlib encoding")]
	Zlib,
	#[error("LZFSE encoding")]
	Lzfse,
	#[error("LZMA2 encoding")]
	Lzma,
	#[error("partitioned image writing (partition-write)")]
	PartitionWrite,
	#[error("parallel encoding")]
	Parallel,
	#[error("compression cache (compression-cache)")]
	CompressionCache,
	#[error("Finder layout")]
	FinderLayout,
	#[error("Finder image background binding")]
	FinderImageBackground,
	#[error("Finder volume icon")]
	FinderVolumeIcon,
	#[error("Finder volume icon badge (badge-icons)")]
	FinderVolumeIconBadge,
	#[error("Finder custom item icon (volume-icon)")]
	FinderCustomIcon,
	#[error("Finder entry flags")]
	FinderFlags,
	#[error("output verification")]
	Verify,
	#[error("atomic path replacement")]
	AtomicReplace,
	#[error("release pipeline")]
	Release,
	#[error("licensed image resources")]
	License,
	#[error("metadata sidecar import")]
	MetadataSidecar,
}

impl Availability {
	pub(super) fn require(self, feature: BuildFeature) -> Result<(), PreflightError> {
		match self {
			Self::Available => Ok(()),
			Self::NotCompiled => Err(PreflightError::FeatureNotCompiled(feature)),
			Self::Unavailable => Err(PreflightError::BackendUnavailable(feature)),
		}
	}
}
