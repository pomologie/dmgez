//! Source metadata and preservation policy (C4).

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod capture;
pub mod policy;
pub mod sidecar;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use capture::{Error, capture_regular};
#[cfg(all(feature = "volume-icon", any(target_os = "linux", target_os = "macos")))]
pub use capture::{ItemIconSourceConflict, probe_item_icon_conflict};

const FINDER_INFO_BYTES: usize = 32;

/// Remaining bounds for a regular file's attribute capture.
#[derive(Clone, Copy, Debug)]
pub struct AttributeLimits {
	/// Aggregate owned attribute bytes, including names and record metadata.
	pub bytes: usize,
	/// Maximum bytes in one attribute value or buffered resource fork.
	pub value_bytes: usize,
	/// Maximum number of names admitted from this handle.
	pub attributes: usize,
}

/// An ordinary extended attribute as exact host name and value bytes.
#[derive(Debug, Eq, PartialEq)]
pub struct Attribute {
	pub name: Vec<u8>,
	pub value: Vec<u8>,
}

/// Source metadata captured separately from the regular data-fork stream.
#[derive(Debug)]
pub struct Attributes {
	pub finder_info: Option<[u8; FINDER_INFO_BYTES]>,
	pub resource_fork: Option<Vec<u8>>,
	pub extended: Vec<Attribute>,
	/// Charged bytes including temporary name storage, suitable for reducing the
	/// containing build's metadata budget before examining another file.
	pub accounted_bytes: usize,
}
