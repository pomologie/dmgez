use std::{
	fmt::Write as _,
	io,
	path::{Path, PathBuf},
};

use crate::project_input;

pub(super) const SHA256_BYTES: usize = 32;
pub(super) const SHA256_HEX_BYTES: usize = SHA256_BYTES * 2;

pub(super) struct Snapshot {
	pub(super) path: PathBuf,
	pub(super) text: String,
	limit: usize,
}

impl Snapshot {
	pub(super) fn open(path: &Path, limit: usize) -> io::Result<Self> {
		let path = path.canonicalize()?;
		let text = project_input::read_limited(&path, limit)?;
		Ok(Self { path, text, limit })
	}

	pub(super) fn validate(&self) -> io::Result<()> {
		let observed = project_input::read_limited(&self.path, self.limit).map_err(|error| {
			io::Error::other(format!(
				"could not recheck edit input {:?}: {error}",
				self.path
			))
		})?;
		if observed != self.text {
			return Err(io::Error::other(format!(
				"edit input changed after it was read: {:?}",
				self.path
			)));
		}
		Ok(())
	}
}

/// Encode a SHA-256 digest as lowercase hexadecimal for an edit report.
#[must_use]
pub fn encode_sha256(digest: &[u8; SHA256_BYTES]) -> String {
	let mut encoded = String::with_capacity(SHA256_HEX_BYTES);
	for byte in digest {
		write!(encoded, "{byte:02x}").expect("writing to a string is infallible");
	}
	encoded
}
