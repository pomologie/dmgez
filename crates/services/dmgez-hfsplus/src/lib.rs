//! Compose dmgez image planning with the versioned HFS+ builder.
//!
//! This crate uses the source and metadata adapters for Linux and macOS.

#![cfg(any(target_os = "linux", target_os = "macos"))]

mod hfsplus;

pub use hfsplus::{
	Error, HfsBuildReport, HfsImage, MetadataLoss, PreparedHfsBuild, capabilities, prepare,
	prepare_with_progress,
};
