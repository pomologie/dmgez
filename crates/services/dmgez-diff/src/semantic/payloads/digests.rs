use std::{collections::HashMap, error::Error as StdError};

use dmgez_vfs::volume::{
	Fork, ForkKind, NodeHandle, StorageId, ViewError, VolumeBackend, VolumeView,
};

use crate::Side;

use super::{CHUNK_BYTES, Contents, Error, PairedPaths, Payloads};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(in crate::semantic) struct ForkDigest {
	pub(in crate::semantic) length: u64,
	hash: [u8; blake3::OUT_LEN],
}

#[derive(Default)]
struct ForkDigests {
	data: Option<ForkDigest>,
	resource: Option<ForkDigest>,
}

/// Digests reused only across aliases in one comparison of immutable views.
#[derive(Default)]
pub(super) struct DigestCache(HashMap<StorageId, ForkDigests>);

impl DigestCache {
	fn lookup(
		&self,
		storage: Option<&StorageId>,
		kind: ForkKind,
		length: u64,
	) -> Option<ForkDigest> {
		let forks = self.0.get(storage?)?;
		let digest = match kind {
			ForkKind::Data => forks.data,
			ForkKind::Resource => forks.resource,
			_ => None,
		};
		digest.filter(|cached| cached.length == length)
	}

	fn insert(&mut self, storage: Option<&StorageId>, kind: ForkKind, digest: ForkDigest) {
		let Some(storage) = storage else {
			return;
		};
		let forks = self.0.entry(storage.clone()).or_default();
		match kind {
			ForkKind::Data => forks.data = Some(digest),
			ForkKind::Resource => forks.resource = Some(digest),
			_ => {}
		}
	}
}

#[derive(Clone, Copy, Default)]
pub(super) struct FingerprintBytes {
	left: u64,
	right: u64,
}

impl FingerprintBytes {
	fn largest(self) -> u64 {
		self.left.max(self.right)
	}
}

impl<F: FnMut() -> bool> Payloads<F> {
	pub(in crate::semantic) fn set_contents(&mut self, contents: Contents) {
		self.contents = contents;
	}

	pub(super) fn compare_digests<L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		left: &mut Fork<'_, L>,
		right: &mut Fork<'_, R>,
		kind: ForkKind,
		paths: PairedPaths<'_>,
		(left_storage, right_storage): (Option<&StorageId>, Option<&StorageId>),
	) -> Result<bool, Error<L, R>> {
		let length = left.len();
		if length != right.len() {
			return Ok(false);
		}
		let left_cached = self.digests.lookup(left_storage, kind, length);
		let right_cached = self.digests.lookup(right_storage, kind, length);
		if left_cached.is_none() || right_cached.is_none() {
			self.charge_bytes(length)?;
		}
		let left_hash = self.resolve_digest(left, left_cached, left_storage, kind, |source| {
			Error::LeftObject {
				path: paths.left.to_vec(),
				source,
			}
		})?;
		let right_hash =
			self.resolve_digest(right, right_cached, right_storage, kind, |source| {
				Error::RightObject {
					path: paths.right.to_vec(),
					source,
				}
			})?;
		if left_hash != right_hash {
			return Ok(false);
		}
		if self.contents == Contents::Blake3Exact {
			return self.compare_forks(left, right, paths);
		}
		Ok(true)
	}

	fn resolve_digest<E: StdError + 'static, L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		fork: &mut Fork<'_, E>,
		cached: Option<ForkDigest>,
		storage: Option<&StorageId>,
		kind: ForkKind,
		read_error: impl Fn(ViewError<E>) -> Error<L, R>,
	) -> Result<ForkDigest, Error<L, R>> {
		if let Some(digest) = cached {
			return Ok(digest);
		}
		let digest = self.read_digest(fork, read_error)?;
		self.digests.insert(storage, kind, digest);
		Ok(digest)
	}

	pub(in crate::semantic) fn fingerprint_left<L: VolumeBackend, R: StdError + 'static>(
		&mut self,
		left: (&mut VolumeView<L>, &NodeHandle<L::Id>),
		kind: ForkKind,
		path: &[String],
	) -> Result<Option<ForkDigest>, Error<L::Error, R>> {
		self.fingerprint(left, kind, Side::Left, |source| Error::LeftObject {
			path: path.to_vec(),
			source,
		})
	}

	pub(in crate::semantic) fn fingerprint_right<L: StdError + 'static, R: VolumeBackend>(
		&mut self,
		right: (&mut VolumeView<R>, &NodeHandle<R::Id>),
		kind: ForkKind,
		path: &[String],
	) -> Result<Option<ForkDigest>, Error<L, R::Error>> {
		self.fingerprint(right, kind, Side::Right, |source| Error::RightObject {
			path: path.to_vec(),
			source,
		})
	}

	fn fingerprint<B: VolumeBackend, L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		(view, node): (&mut VolumeView<B>, &NodeHandle<B::Id>),
		kind: ForkKind,
		side: Side,
		read_error: impl Fn(ViewError<B::Error>) -> Error<L, R>,
	) -> Result<Option<ForkDigest>, Error<L, R>> {
		self.poll()?;
		view.with_fork(node, kind, |fork| {
			self.reserve_fingerprint(side, fork.len())?;
			self.read_digest(fork, &read_error)
		})
		.map_err(read_error)?
		.transpose()
	}

	// The caller must reserve the fork's logical length before reading it.
	fn read_digest<E: StdError + 'static, L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		fork: &mut Fork<'_, E>,
		read_error: impl Fn(ViewError<E>) -> Error<L, R>,
	) -> Result<ForkDigest, Error<L, R>> {
		let length = fork.len();
		let mut hasher = blake3::Hasher::new();
		let mut offset = 0;
		while offset < length {
			self.poll()?;
			let width = (length - offset).min(CHUNK_BYTES as u64);
			let chunk = &mut self.left_scratch[..width as usize];
			fork.read_exact_at(offset, chunk).map_err(&read_error)?;
			hasher.update(chunk);
			offset += width;
		}
		Ok(ForkDigest {
			length,
			hash: *hasher.finalize().as_bytes(),
		})
	}

	fn reserve_fingerprint<L: StdError + 'static, R: StdError + 'static>(
		&mut self,
		side: Side,
		length: u64,
	) -> Result<(), Error<L, R>> {
		let mut next = self.fingerprint_bytes;
		let total = match side {
			Side::Left => &mut next.left,
			Side::Right => &mut next.right,
		};
		*total = total
			.checked_add(length)
			.ok_or(Error::ByteLimit { limit: self.limit })?;
		// The shared allowance is per side. Charge only growth in the larger
		// cumulative total for unpaired reads.
		self.charge_bytes(next.largest() - self.fingerprint_bytes.largest())?;
		self.fingerprint_bytes = next;
		Ok(())
	}
}
