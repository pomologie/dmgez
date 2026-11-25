use std::collections::TryReserveError;

#[cfg(not(windows))]
use dmgez_builder::metadata as host_metadata;
#[cfg(feature = "finder-write")]
use dmgez_builder::presentation::signed_targets;
use dmgez_builder::{
	execution::ExecuteError,
	metadata::{policy, sidecar},
	planning,
	preflight::PreflightError,
};
use hfsplus_builder::volume::{CapacityError, VolumeError};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Prepare(#[from] planning::Error),
	#[error(transparent)]
	Filesystem(#[from] VolumeError),
	#[error(transparent)]
	Capacity(#[from] CapacityError),
	#[error(transparent)]
	Profile(#[from] PreflightError),
	#[error(transparent)]
	Execute(#[from] ExecuteError),
	#[cfg(feature = "finder-write")]
	#[error(transparent)]
	Finder(#[from] dmgez_builder::presentation::Error),
	#[cfg(feature = "finder-write")]
	#[error("a reserved Finder background identity is missing at {path:?}")]
	BackgroundIdentity { path: String },
	#[cfg(feature = "finder-write")]
	#[error("the stored HFS+ volume name cannot be represented in a Finder alias")]
	BackgroundVolumeName(#[source] std::string::FromUtf16Error),
	#[error("build was cancelled while sealing the HFS+ image")]
	Cancelled,
	#[error("build metadata exceeds the available byte budget")]
	MetadataBudget,
	#[error("could not reserve bounded adapter metadata")]
	Reserve(#[from] TryReserveError),
	#[error("HFS+ adapter received an unsupported build profile")]
	UnsupportedProfile,
	#[error("virtual path is outside the HFS+ manifest profile: {path:?}")]
	Path { path: String },
	#[error("source metadata policy failed for {path:?}")]
	Policy {
		path: String,
		#[source]
		cause: policy::Error,
	},
	#[error("hard-link grouping failed")]
	HardLinks(#[source] policy::Error),
	#[error("hard-link source validation failed")]
	LinkSource(#[source] dmgez_builder::manifest::Error),
	#[error("hard-link aliases have inconsistent source metadata at {path:?}")]
	LinkMetadata { path: String },
	#[cfg(feature = "finder-write")]
	#[error("hard-link aliases request contradictory Finder flags at {path:?}")]
	LinkFinderFlags { path: String },
	#[cfg(not(windows))]
	#[error("could not capture regular-source metadata for {path:?}")]
	Capture {
		path: String,
		#[source]
		cause: host_metadata::Error,
	},
	#[error("could not merge the AppleDouble sidecar for {path:?}")]
	Sidecar {
		path: String,
		#[source]
		cause: sidecar::Error,
	},
	#[error("unsupported source kind or missing source at {path:?}")]
	UnsupportedEntry { path: String },
	#[error("requested Finder flag overrides are unavailable at {path:?}")]
	FinderFlags { path: String },
	#[cfg(feature = "finder-write")]
	#[error("could not index Finder flag targets")]
	FinderFlagTargetIndex(#[source] signed_targets::Error),
	#[cfg(feature = "finder-write")]
	#[error("could not verify the Finder flag target at {path:?}")]
	FinderFlagTarget {
		path: String,
		#[source]
		source: signed_targets::Error,
	},
	#[cfg(feature = "finder-write")]
	#[error("cannot set Finder flags on {protection}: {path:?}")]
	FinderFlagsProtected {
		path: String,
		protection: signed_targets::Protection,
	},
	#[cfg(feature = "finder-write")]
	#[error("cannot set Finder flags on an item with code-signing attributes: {path:?}")]
	FinderFlagsSigningMetadata { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon cannot be applied to the item at {path:?}")]
	CustomIconTarget { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon cannot change shared hard-link metadata at {path:?}")]
	CustomIconHardLink { path: String },
	#[cfg(feature = "volume-icon")]
	#[error("a custom Finder icon would replace an existing resource fork at {path:?}")]
	CustomIconResourceFork { path: String },
	#[cfg(all(feature = "volume-icon", not(windows)))]
	#[error("a custom Finder icon would invalidate native code-signing metadata at {path:?}")]
	CustomIconSigningMetadata { path: String },
	#[error("HFS+ timestamp exceeds the representable epoch")]
	Timestamp,
	#[error("admitted build has no output path")]
	OutputPath,
	#[error("admitted attribute name cannot be represented in HFS+")]
	AttributeName(#[from] std::str::Utf8Error),
}
