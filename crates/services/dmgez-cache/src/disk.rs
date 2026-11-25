use std::{io, num::NonZeroU64, path::Path};

use dmgez_udif::writer::cache::{CacheKey, ChunkCache};
use thiserror::Error;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod posix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use posix as platform;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use unsupported as platform;

/// Device and inode of a held cache directory or one of its ancestors.
pub type DirectoryIdentity = (u64, u64);

/// Failure to admit a cache directory.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("persistent compression caching is unavailable on this host")]
	UnsupportedHost,
	#[error(
		"cache directory must be an absolute path below the filesystem root without parent components"
	)]
	Path,
	#[error(
		"cache directory must be owned by the effective user and have mode 0700 with no extended ACL"
	)]
	Privacy,
	#[error("could not open or verify the cache directory")]
	Directory(#[from] io::Error),
}

/// A best-effort compression cache on a local macOS or Linux filesystem.
///
/// Directory and entry access is scoped to held descriptors. Writers using
/// this backend coordinate with advisory locks and enforce their configured
/// byte cap and an entry-count limit. Stores evict least recently used entries
/// using file modification times. Entries are disposable across crashes, and
/// contention or failed operations leave the encoder free to regenerate data.
#[derive(Debug)]
pub struct DiskCache {
	platform: platform::Cache,
}

impl DiskCache {
	/// Whether this build includes a platform backend.
	pub const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

	/// Opens an existing private directory without altering it.
	///
	/// Paths must be absolute. Symlinks and parent components are refused.
	/// The caller must keep the cache separate from inputs and outputs and
	/// use the same byte cap for cooperating writers using this directory.
	///
	/// # Errors
	///
	/// Returns an error when a host is unsupported or the directory cannot be
	/// held and verified as private.
	pub fn open(path: &Path, max_bytes: NonZeroU64) -> Result<Self, Error> {
		Ok(Self {
			platform: platform::Cache::open(path, max_bytes)?,
		})
	}

	/// Returns the held directory's device and inode.
	#[must_use]
	pub fn directory_identity(&self) -> DirectoryIdentity {
		self.platform.directory_identity()
	}

	/// Returns the traversal identities from the filesystem root to this directory.
	#[must_use]
	pub fn ancestors(&self) -> &[DirectoryIdentity] {
		self.platform.ancestors()
	}
}

impl ChunkCache for DiskCache {
	fn load(&self, key: CacheKey, output: &mut [u8]) -> Option<usize> {
		self.platform.load(key.as_bytes(), output)
	}

	fn store(&self, key: CacheKey, encoded: &[u8]) {
		self.platform.store(key.as_bytes(), encoded);
	}
}
