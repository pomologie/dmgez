//! Strict parsing and semantic admission for versioned dmgez project manifests.

mod content;
mod finder;
mod model;
mod overrides;
mod parse;
mod release;
mod value;

pub use content::{Content, ContentError, CopySource, Payload};
pub use finder::{
	Arrangement, Background, Column, Direction, Finder, FinderOptions, Folder, IconSlot, IconView,
	LabelPosition, ListView, View, VolumeIcon, VolumeIconError,
};
pub use model::{
	Build, Capacity, Compression, CompressionError, Filesystem, Image, ImageError, ImageFormat,
	Metadata, MetadataPolicy, Partition, Performance, PerformanceError, Project,
};
pub use overrides::{ConfigOverride, OverrideError};
pub use parse::{ConfigError, DefineError, MAX_MANIFEST_BYTES, SCHEMA_VERSION};
pub use release::{
	AppSigning, Backend, Buttons, License, Notarization, Release, Signing, SigningRule,
};
pub use value::{
	AutoFlag, Bytes, Coordinate, Dimension, LocalPath, Size, SizeError, Text, VirtualPath,
	VirtualPathError,
};
