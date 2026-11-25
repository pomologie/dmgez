//! Strict parsing and semantic admission for versioned dmgez project manifests.

#[cfg(feature = "appdmg-import")]
mod appdmg;
mod content;
mod finder;
mod model;
mod overrides;
mod parse;
#[cfg(feature = "project-edit")]
pub mod project_edit;
mod release;
mod value;

#[cfg(feature = "appdmg-import")]
pub use appdmg::{AppDmgDiagnostic, AppDmgImport, AppDmgImportError, import_appdmg};
pub use content::{Content, ContentError, CopySource, Payload};
pub use finder::{
	Arrangement, Background, BackgroundColorError, BackgroundTemplate, BackgroundTemplateError,
	Column, Direction, Finder, FinderOptions, Folder, IconSlot, IconView, LabelPosition, ListView,
	View, VolumeIcon, VolumeIconError, parse_solid_background_color,
};
pub use model::{
	Build, Capacity, Compression, CompressionCache, CompressionError, Filesystem, Image,
	ImageError, ImageFormat, Metadata, MetadataPolicy, Partition, Performance, PerformanceError,
	Project,
};
pub use overrides::{ConfigOverride, OverrideError};
pub use parse::{ConfigError, DefineError, MAX_MANIFEST_BYTES, SCHEMA_VERSION};
pub use release::{
	AppSigning, Backend, Buttons, License, Notarization, Release, Signing, SigningMode, SigningRule,
};
pub use value::{
	AutoFlag, Bytes, Coordinate, Dimension, LocalPath, Size, SizeError, Text, VirtualPath,
	VirtualPathError,
};
