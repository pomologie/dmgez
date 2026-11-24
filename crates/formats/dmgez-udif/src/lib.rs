//! Bounded, native UDIF disk image reading and streaming construction.
//!
//! The supported profile is single-file, unsigned UDIF v4 with 512-byte
//! logical sectors. Filesystem bytes are opaque to this crate.

#[cfg(any(feature = "read", feature = "write"))]
mod binary;
#[cfg(any(feature = "read", feature = "write"))]
mod blkx;
#[cfg(any(feature = "read", feature = "write"))]
mod checksum;
#[cfg(all(any(feature = "read", feature = "write"), feature = "zlib"))]
mod codec;
mod error;
#[cfg(feature = "read")]
mod index;
#[cfg(any(feature = "read", feature = "write"))]
mod resource;
#[cfg(any(feature = "read", feature = "write"))]
mod trailer;
#[cfg(any(feature = "read", all(test, feature = "write"), kani))]
mod validation;

#[cfg(feature = "read")]
pub mod reader;
#[cfg(feature = "write")]
pub mod writer;

pub use error::Error;

/// UDIF logical sector size in bytes, independent of filesystem block size.
pub const SECTOR_BYTES: u64 = 512;
