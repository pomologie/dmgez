use std::{num::NonZeroU64, path::Path};

use super::{DirectoryIdentity, Error};

#[derive(Debug)]
pub(super) struct Cache {
	unavailable: std::convert::Infallible,
}

impl Cache {
	pub(super) fn open(_path: &Path, _max_bytes: NonZeroU64) -> Result<Self, Error> {
		Err(Error::UnsupportedHost)
	}

	pub(super) fn directory_identity(&self) -> DirectoryIdentity {
		match self.unavailable {}
	}

	pub(super) fn ancestors(&self) -> &[DirectoryIdentity] {
		match self.unavailable {}
	}

	pub(super) fn load(&self, _key: &[u8; 32], _output: &mut [u8]) -> Option<usize> {
		match self.unavailable {}
	}

	pub(super) fn store(&self, _key: &[u8; 32], _encoded: &[u8]) {
		match self.unavailable {}
	}
}
