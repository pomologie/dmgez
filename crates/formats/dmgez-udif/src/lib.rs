//! Bounded, native UDIF disk image reading.
//!
//! The supported profile is single-file, unsigned UDIF v4 with 512-byte
//! logical sectors. Filesystem bytes are opaque to this crate.

#[cfg(feature = "read")]
mod binary;
#[cfg(feature = "read")]
mod blkx;
#[cfg(feature = "read")]
mod checksum;
#[cfg(all(feature = "read", feature = "zlib"))]
mod codec;
mod error;
#[cfg(feature = "read")]
mod index;
#[cfg(feature = "read")]
mod resource;
#[cfg(feature = "read")]
mod trailer;
#[cfg(any(feature = "read", kani))]
mod validation;

#[cfg(feature = "read")]
pub mod reader;

pub use error::Error;

/// UDIF logical sector size in bytes, independent of filesystem block size.
pub const SECTOR_BYTES: u64 = 512;
