use thiserror::Error;

/// A failure in UDIF admission, encoding, decoding or verification.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	/// An input does not identify itself as UDIF.
	#[error("invalid format: {0}")]
	InvalidFormat(&'static str),
	/// A recognized UDIF variant lies outside the implemented profile.
	#[error("unsupported UDIF variant: {0}")]
	Unsupported(&'static str),
	/// A recognized block map uses an unsupported type code.
	#[error("unsupported UDIF run type {kind:#010x} at logical sector {sector}")]
	UnsupportedRun { kind: u32, sector: u64 },
	/// The build omitted an optional codec.
	#[error("feature not compiled: {0}")]
	FeatureNotCompiled(&'static str),
	/// The image contradicts the format or its own declared geometry.
	#[error("corrupt UDIF: {0}")]
	Corrupt(&'static str),
	/// The compressed stream did not match its declared physical and logical ranges.
	#[error("invalid zlib stream at physical byte {offset} for logical byte {logical_offset}")]
	InvalidZlib { offset: u64, logical_offset: u64 },
	/// The LZFSE stream did not match its declared physical and logical ranges.
	#[error("invalid LZFSE stream at physical byte {offset} for logical byte {logical_offset}")]
	InvalidLzfse { offset: u64, logical_offset: u64 },
	/// An input or configured operation exceeds an admitted resource budget.
	#[error("resource limit: {0}")]
	ResourceLimit(&'static str),
	/// A byte request falls outside the logical disk.
	#[error("logical byte range {offset} + {len} exceeds disk size {disk_len}")]
	OutOfRange {
		offset: u64,
		len: u64,
		disk_len: u64,
	},
	/// The writer needs a nonempty source aligned to 512-byte sectors.
	#[error("raw source length must be nonzero and a multiple of 512 bytes")]
	UnalignedSource,
	/// The source length disagrees with the writer's declared geometry.
	#[error("raw source {0}")]
	SourceLength(&'static str),
	/// The backing source changed after it was admitted.
	#[error("source changed while reading")]
	SourceChanged,
	/// The caller requested cancellation.
	#[error("UDIF write cancelled")]
	Cancelled,
	/// The compression primitive failed to encode an admitted block.
	#[cfg(all(feature = "write", feature = "zlib"))]
	#[error("zlib compression failed")]
	Compression(#[source] flate2::CompressError),
	/// A private parallel worker pool could not start.
	#[cfg(all(feature = "write", feature = "parallel"))]
	#[error("could not start UDIF workers")]
	WorkerPool(#[source] rayon::ThreadPoolBuildError),
	/// An emitted or decoded resource is not a valid property list.
	#[cfg(any(feature = "read", feature = "write"))]
	#[error("UDIF resource property list is invalid")]
	Resource(#[source] plist::Error),
	/// Computed CRC32 differs from the one declared in the image.
	#[error("{domain} checksum mismatch: expected {expected:08x}, got {actual:08x}")]
	ChecksumMismatch {
		domain: String,
		expected: u32,
		actual: u32,
	},
	/// A read or write failed.
	#[error("UDIF I/O: {0}")]
	Io(#[from] std::io::Error),
}
