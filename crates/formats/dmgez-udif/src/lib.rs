//! Bounded, native UDIF disk image reading and streaming construction.
//!
//! The supported profile is single-file, unsigned UDIF v4 with 512-byte
//! logical sectors. Filesystem bytes are opaque to this crate.

#[cfg(any(feature = "read", feature = "write"))]
mod binary;
#[cfg(any(feature = "read", feature = "write"))]
mod blkx;
mod cancellation;
#[cfg(any(feature = "read", feature = "write"))]
mod checksum;
#[cfg(all(any(feature = "read", feature = "write"), feature = "zlib"))]
mod codec;
mod error;
mod geometry;
#[cfg(feature = "read")]
mod index;
#[cfg(all(any(feature = "read", feature = "write"), feature = "lzfse"))]
mod lzfse;
#[cfg(all(any(feature = "read", feature = "write"), feature = "lzma"))]
mod lzma;
#[cfg(any(feature = "read", feature = "write"))]
mod resource;
mod signature_append;
mod signature_directory;
#[cfg(feature = "signature-integrity")]
mod signature_integrity;
mod signature_ticket;
#[cfg(any(feature = "read", feature = "write"))]
mod trailer;
mod trailer_header;
#[cfg(any(feature = "read", all(test, feature = "write"), kani))]
mod validation;

#[cfg(feature = "read")]
pub mod reader;
#[cfg(feature = "write")]
pub mod writer;

pub use cancellation::Cancellation;
pub use error::Error;
pub use geometry::SECTOR_BYTES;
pub use signature_append::{
	portable_signing_limit, signed_signature_range, trailer_for_signature_digest,
	validate_signature_append, validate_staple_layout, validate_unsigned_signature_fields,
};
pub use signature_directory::{EmbeddedCodeDirectory, embedded_code_directory};
#[cfg(feature = "signature-integrity")]
pub use signature_integrity::{
	SignatureIntegrityError, read_embedded_signature, verify_code_directory_digests,
	verify_signature_integrity,
};
pub use signature_ticket::{
	MAX_EMBEDDED_SIGNATURE_BYTES, has_stapled_ticket, validate_ticket_addition,
};
pub use trailer_header::{TRAILER_BYTES, check_trailer_header};

#[cfg(kani)]
#[path = "../verification/kani/checked_range.rs"]
mod checked_range_proof;
