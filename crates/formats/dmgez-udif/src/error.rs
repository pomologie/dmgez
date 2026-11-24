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
	/// The backing source changed after it was admitted.
	#[error("source changed while reading")]
	SourceChanged,
	/// A decoded resource is not a valid property list.
	#[cfg(feature = "read")]
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
