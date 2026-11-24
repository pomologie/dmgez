//! Planning and execution for file-tree disk image builds.

pub mod execution;
pub mod manifest;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod metadata;
pub mod planning;
pub mod preflight;
#[cfg(feature = "finder-write")]
pub mod presentation;
pub mod progress;
