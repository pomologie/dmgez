use dmgez_config::{VirtualPath, VirtualPathError};
use thiserror::Error;

#[cfg(feature = "volume-icon")]
use super::item_icon;

/// Finder assets could not be prepared for the resolved image tree.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("Finder layout folder is absent: {folder:?}")]
	MissingFolder { folder: String },
	#[error("Finder layout target is not a folder: {folder:?}")]
	NotDirectory { folder: String },
	#[error("Finder layout folder is ambiguous under HFS+ comparison: {folder:?}")]
	AmbiguousFolder { folder: String },
	#[error("content conflicts with the reserved Finder store: {path:?}")]
	StoreCollision { path: VirtualPath },
	#[error("Finder metadata would exceed the build's entry limit")]
	EntryBudget,
	#[error("Finder metadata would exceed the build's metadata byte limit")]
	MetadataBudget,
	#[error("cannot reserve Finder planning memory")]
	Reserve(#[from] std::collections::TryReserveError),
	#[error(transparent)]
	SignedTargets(#[from] super::signed_targets::Error),
	#[error("cannot represent a Finder store path in {folder:?}")]
	StorePath {
		folder: String,
		#[source]
		source: VirtualPathError,
	},
	#[error("cannot generate Finder layout for {folder:?}")]
	Layout {
		folder: String,
		#[source]
		source: dmgez_finder::Error,
	},
	#[error("Finder metadata generation was cancelled")]
	Cancelled,
	#[error("content conflicts with the reserved Finder background directory: {path:?}")]
	BackgroundCollision { path: VirtualPath },
	#[error("Finder background must contain 1 to {max} encoded bytes", max = super::background::MAX_FILE_BYTES)]
	BackgroundFileSize,
	#[error("Finder background exceeds the admitted working memory or dimensions")]
	BackgroundMemory,
	#[error("animated images are unsupported as Finder backgrounds")]
	BackgroundAnimated,
	#[error("Finder background source failed its identity checks")]
	BackgroundSource(#[source] crate::manifest::Error),
	#[error("could not read the captured Finder background")]
	BackgroundRead(#[source] dmgez_core::source::SourceError),
	#[error("Finder background must be a valid PNG")]
	BackgroundPng(#[source] png::DecodingError),
	#[error("Finder background must be PNG, TIFF, JPEG, BMP, static GIF, or static WebP")]
	BackgroundImageFormat,
	#[error("could not decode the Finder background BMP")]
	BackgroundBmp(#[source] image::ImageError),
	#[error("could not decode the Finder background GIF")]
	BackgroundGif(#[source] gif::DecodingError),
	#[error("could not decode the Finder background JPEG")]
	BackgroundJpeg(#[source] zune_jpeg::errors::DecodeErrors),
	#[error("could not decode the Finder background WebP")]
	BackgroundWebp(#[source] image_webp::DecodingError),
	#[error("Retina Finder background must be twice the base width and height")]
	BackgroundRetinaDimensions,
	#[error("Retina Finder background PNG representation is unsupported")]
	BackgroundRetinaPngFormat,
	#[error("Finder background TIFF could not be encoded or decoded")]
	BackgroundTiff(#[source] tiff::TiffError),
	#[error(
		"Finder background TIFF must contain a supported single image or a 1x and 2x pair with matching logical sizes"
	)]
	BackgroundTiffRepresentation,
	#[error("Finder background alias names a different path than the captured image")]
	BackgroundTargetMismatch,
	#[cfg(feature = "volume-icon")]
	#[error("content conflicts with the reserved volume icon: {path:?}")]
	VolumeIconCollision { path: VirtualPath },
	#[cfg(feature = "volume-icon")]
	#[error("volume icon validation exceeds the available working memory")]
	VolumeIconMemory,
	#[cfg(feature = "volume-icon")]
	#[error("volume icon source failed its identity checks")]
	VolumeIconSource(#[from] crate::manifest::Error),
	#[cfg(feature = "volume-icon")]
	#[error("could not read the captured volume icon")]
	VolumeIconRead(#[from] dmgez_core::source::SourceError),
	#[cfg(feature = "volume-icon")]
	#[error("could not prepare the Finder volume icon")]
	VolumeIcon(#[from] dmgez_finder::volume_icon::Error),
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon requires a regular file or Unicode HFS+ folder: {path:?}")]
	ItemIconTarget { path: VirtualPath },
	#[cfg(feature = "volume-icon")]
	#[error(
		"cannot add a custom Finder icon to a protected application bundle or framework: {path:?}"
	)]
	ItemIconSignedBundle { path: VirtualPath },
	#[cfg(feature = "volume-icon")]
	#[error(
		"cannot add a custom Finder icon when executable signature integrity is unverified: {path:?}"
	)]
	ItemIconSignedExecutable { path: VirtualPath },
	#[cfg(feature = "volume-icon")]
	#[error("could not read the custom Finder icon target to check its signature: {path:?}")]
	ItemIconTargetRead {
		path: VirtualPath,
		#[source]
		source: dmgez_core::source::SourceError,
	},
	#[cfg(feature = "volume-icon")]
	#[error("custom Finder icon validation exceeds the available working memory")]
	ItemIconMemory,
	#[cfg(feature = "volume-icon")]
	#[error("custom Finder icon source failed its identity checks")]
	ItemIconSource(#[source] crate::manifest::Error),
	#[cfg(feature = "volume-icon")]
	#[error("could not read the captured custom Finder icon")]
	ItemIconRead(#[source] dmgez_core::source::SourceError),
	#[cfg(feature = "volume-icon")]
	#[error("could not prepare the custom Finder icon for {path:?}")]
	ItemIcon {
		path: VirtualPath,
		#[source]
		source: item_icon::Error,
	},
}
