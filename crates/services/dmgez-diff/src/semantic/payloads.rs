use std::error::Error as StdError;

#[cfg(feature = "content-hash")]
use dmgez_vfs::volume::StorageId;
use dmgez_vfs::volume::{Fork, ForkKind, NodeHandle, VolumeBackend, VolumeView};

use crate::Side;

use super::Error;

#[cfg(feature = "content-hash")]
use super::Contents;
#[cfg(feature = "content-hash")]
pub(super) use digests::ForkDigest;
#[cfg(feature = "content-hash")]
use digests::{DigestCache, FingerprintBytes};

#[cfg(feature = "content-hash")]
mod digests;

const CHUNK_BYTES: usize = 64 * 1024;

pub(super) struct Payloads<F> {
	limit: u64,
	compared_bytes: u64,
	cancelled: F,
	left_scratch: Vec<u8>,
	right_scratch: Vec<u8>,
	#[cfg(feature = "content-hash")]
	contents: Contents,
	#[cfg(feature = "content-hash")]
	digests: DigestCache,
	#[cfg(feature = "content-hash")]
	fingerprint_bytes: FingerprintBytes,
}

impl<F: FnMut() -> bool> Payloads<F> {
	pub(super) fn new(limit: u64, cancelled: F) -> Self {
		let scratch_bytes = limit.min(CHUNK_BYTES as u64) as usize;
		Self {
			limit,
			compared_bytes: 0,
			cancelled,
			left_scratch: vec![0; scratch_bytes],
			right_scratch: vec![0; scratch_bytes],
			#[cfg(feature = "content-hash")]
			contents: Contents::Exact,
			#[cfg(feature = "content-hash")]
			digests: DigestCache::default(),
			#[cfg(feature = "content-hash")]
			fingerprint_bytes: FingerprintBytes::default(),
		}
	}

	pub(super) const fn compared_bytes(&self) -> u64 {
		self.compared_bytes
	}

	pub(super) fn charge_bytes<L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		read_bytes: u64,
	) -> Result<(), Error<L, R>> {
		if read_bytes > self.limit - self.compared_bytes {
			return Err(Error::ByteLimit { limit: self.limit });
		}
		self.compared_bytes += read_bytes;
		Ok(())
	}

	pub(super) fn poll<L: StdError + 'static, R: StdError + 'static>(
		&mut self,
	) -> Result<(), Error<L, R>> {
		if (self.cancelled)() {
			return Err(Error::Cancelled);
		}
		Ok(())
	}

	pub(super) fn fork<L: VolumeBackend, R: VolumeBackend>(
		&mut self,
		left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
		right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
		kind: ForkKind,
		path: &[String],
		#[cfg(feature = "content-hash")] storage_groups: (Option<&StorageId>, Option<&StorageId>),
	) -> Result<bool, Error<L::Error, R::Error>> {
		self.poll()?;
		let (left, left_node) = left;
		let (right, right_node) = right;
		let left_result = left
			.with_fork(left_node, kind, |left_fork| {
				right.with_fork(right_node, kind, |right_fork| {
					#[cfg(feature = "content-hash")]
					if self.contents != Contents::Exact {
						return self.compare_digests(
							left_fork,
							right_fork,
							kind,
							path,
							storage_groups,
						);
					}
					self.compare_forks(left_fork, right_fork, path)
				})
			})
			.map_err(|source| Error::LeftObject {
				path: path.to_vec(),
				source,
			})?;
		let required = kind == ForkKind::Data;
		if let Some(right_result) = left_result {
			let right_result = right_result.map_err(|source| Error::RightObject {
				path: path.to_vec(),
				source,
			})?;
			return match right_result {
				Some(compared) => compared,
				None if required => Err(missing_data(Side::Right, path)),
				None => Ok(false),
			};
		}
		if required {
			return Err(missing_data(Side::Left, path));
		}
		let right_result = right
			.with_fork(right_node, kind, |_| ())
			.map_err(|source| Error::RightObject {
				path: path.to_vec(),
				source,
			})?;
		Ok(right_result.is_none())
	}

	pub(super) fn compare_forks<L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		left: &mut Fork<'_, L>,
		right: &mut Fork<'_, R>,
		path: &[String],
	) -> Result<bool, Error<L, R>> {
		let length = left.len();
		if length != right.len() {
			return Ok(false);
		}
		let mut offset = 0;
		while offset < length {
			self.poll()?;
			let remaining = self.limit - self.compared_bytes;
			let width = (length - offset).min(remaining).min(CHUNK_BYTES as u64);
			if width == 0 {
				return Err(Error::ByteLimit { limit: self.limit });
			}
			let left_bytes = &mut self.left_scratch[..width as usize];
			let right_bytes = &mut self.right_scratch[..width as usize];
			left.read_exact_at(offset, left_bytes)
				.map_err(|source| Error::LeftObject {
					path: path.to_vec(),
					source,
				})?;
			right
				.read_exact_at(offset, right_bytes)
				.map_err(|source| Error::RightObject {
					path: path.to_vec(),
					source,
				})?;
			self.compared_bytes += width;
			if left_bytes != right_bytes {
				return Ok(false);
			}
			offset += width;
		}
		Ok(true)
	}

	pub(super) fn link<L: VolumeBackend, R: VolumeBackend>(
		&mut self,
		left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
		right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
		path: &[String],
	) -> Result<bool, Error<L::Error, R::Error>> {
		self.poll()?;
		let left = left
			.0
			.read_link(left.1)
			.map_err(|source| Error::LeftObject {
				path: path.to_vec(),
				source,
			})?;
		let right = right
			.0
			.read_link(right.1)
			.map_err(|source| Error::RightObject {
				path: path.to_vec(),
				source,
			})?;
		// VFS supplies bounded whole link targets. Account both reads even when
		// the lengths alone would establish the difference.
		self.charge_bytes(left.len().max(right.len()) as u64)?;
		Ok(left == right)
	}
}

pub(super) fn missing_data<L: StdError + 'static, R: StdError + 'static>(
	side: Side,
	path: &[String],
) -> Error<L, R> {
	Error::MissingDataFork {
		side,
		path: path.to_vec(),
	}
}
