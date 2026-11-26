//! Build and read disk images using the same engine as the dmgez command.
//!
//! In a composed HFS+ build, `dmgbuilder` plans and builds images from TOML
//! projects. The `inspect` feature provides the `reader` module for logical byte
//! reads and container checksum checks.

#[cfg(feature = "project-build")]
pub mod dmgbuilder;
#[cfg(feature = "inspect")]
pub mod reader;
