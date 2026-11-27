//! Build and read disk images using the same engine as the dmgez command.
//!
//! The crates.io package includes `dmgbuilder` for planning and building HFS+
//! images from TOML projects and `reader` for reading logical bytes and
//! verifying container checksums.

#[cfg(feature = "project-build")]
pub mod dmgbuilder;
#[cfg(feature = "inspect")]
pub mod reader;
