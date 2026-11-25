//! Compose dmgez image planning with the versioned HFS+ builder.
//!
//! This crate uses the source adapters for Linux, macOS and Windows.

#![cfg(any(target_os = "linux", target_os = "macos", windows))]

mod hfsplus;
#[cfg(feature = "image-edit")]
pub mod rebuild;

pub use hfsplus::{
	Error, HfsBuildReport, HfsImage, MetadataLoss, PreparedHfsBuild, capabilities, prepare,
	prepare_with_progress,
};
