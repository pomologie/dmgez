//! Optional content-addressed reuse of bounded UDZO zlib runs.

use std::{
	io::{Read, Write},
	sync::atomic::{AtomicUsize, Ordering},
};

use sha2::{Digest, Sha256};

use crate::{Error, codec};

use super::{OutputMetadata, WriteOptions, WriteReport, write_stream};

// Revise this domain when the qualified encoder backend or encoding policy changes.
// It identifies flate2 1.1.11 / zlib-rs 0.6.8, the RFC 1950 wrapper and the UDZO run.
const ENCODER_REVISION: &[u8] = b"dmgez-udif/udzo/rfc1950/flate2-1.1.11/zlib-rs-0.6.8/v1";
const KEY_BYTES: usize = 32;
const NO_COMPRESSION_LEVEL: u8 = 0;

/// Digest of the qualified encoder identity, zlib level, logical length and raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey([u8; KEY_BYTES]);

impl CacheKey {
	/// The SHA-256 key in its fixed-length binary representation.
	#[must_use]
	pub const fn as_bytes(&self) -> &[u8; KEY_BYTES] {
		&self.0
	}

	fn for_chunk(raw: &[u8], level: u8) -> Self {
		let mut digest = Sha256::new();
		digest.update(ENCODER_REVISION);
		digest.update([level]);
		digest.update((raw.len() as u64).to_be_bytes());
		digest.update(raw);
		Self(digest.finalize().into())
	}
}

/// Best-effort storage for independently verified zlib runs.
///
/// A backend may be called concurrently and must impose its own storage and
/// memory limits. An unavailable entry is a miss and a failed store is optional.
pub trait ChunkCache: Sync {
	/// Copies a complete candidate to the provided buffer and returns its length.
	///
	/// The writer rejects a length beyond this buffer. A backend can signal an
	/// oversized candidate with its length alone, without reading that payload.
	/// Return `None` when an entry is unavailable. Never present a partial read
	/// as a complete candidate.
	fn load(&self, key: CacheKey, output: &mut [u8]) -> Option<usize>;

	/// Offers a complete writer-produced zlib stream for this key.
	fn store(&self, key: CacheKey, encoded: &[u8]);
}

/// Reuse outcomes for eligible nonzero chunks in a successful cached write.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CacheStats {
	/// Candidates that matched the complete logical bytes and were reused.
	pub hits: usize,
	/// Eligible chunks encoded afresh, including rejected candidates.
	pub misses: usize,
	/// Misses caused by a supplied candidate that failed validation.
	pub rejected: usize,
}

/// Wrap a finite logical disk as UDZO, with explicit opt-in to block reuse.
///
/// Only nonzero chunks at a positive compression level probe the cache. Each
/// hit must be beneficial and decode exactly to the original bytes with no
/// trailing data. Failed candidates are encoded afresh. Entries emitted by the
/// same qualified encoder reproduce its byte stream. A backend that supplies an
/// alternative valid encoding can change physical bytes while preserving the
/// full logical disk. Use [write_image](super::write_image) when canonical
/// physical bytes are required independently of cache contents.
///
/// # Errors
/// Rejects non-UDZO profiles, invalid lengths or settings, exhausted resources,
/// cancellation and failed image I/O. The sink may contain an incomplete prefix.
pub fn write_image_with_cache(
	source: impl Read,
	logical_bytes: u64,
	sink: impl Write,
	options: &WriteOptions,
	cache: &dyn ChunkCache,
) -> Result<(WriteReport, CacheStats), Error> {
	let cache = ActiveCache {
		backend: cache,
		hits: AtomicUsize::new(0),
		misses: AtomicUsize::new(0),
		rejected: AtomicUsize::new(0),
	};
	let report = write_stream(
		source,
		logical_bytes,
		sink,
		options,
		OutputMetadata::default(),
		Some(&cache),
	)?;
	Ok((report, cache.stats()))
}

pub(super) struct ActiveCache<'a> {
	backend: &'a dyn ChunkCache,
	hits: AtomicUsize,
	misses: AtomicUsize,
	rejected: AtomicUsize,
}

impl ActiveCache<'_> {
	fn lookup(&self, key: CacheKey, raw: &[u8]) -> Option<Vec<u8>> {
		let mut encoded = vec![0; raw.len() - 1];
		if let Some(len) = self.backend.load(key, &mut encoded) {
			if encoded
				.get(..len)
				.is_some_and(|bytes| codec::matches_inflated(bytes, raw))
			{
				self.hits.fetch_add(1, Ordering::Relaxed);
				encoded.truncate(len);
				return Some(encoded);
			}
			self.rejected.fetch_add(1, Ordering::Relaxed);
		}
		self.misses.fetch_add(1, Ordering::Relaxed);
		None
	}

	fn stats(&self) -> CacheStats {
		CacheStats {
			hits: self.hits.load(Ordering::Relaxed),
			misses: self.misses.load(Ordering::Relaxed),
			rejected: self.rejected.load(Ordering::Relaxed),
		}
	}
}

pub(super) fn deflate(
	raw: &[u8],
	level: u8,
	cache: Option<&ActiveCache<'_>>,
) -> Result<Option<Vec<u8>>, Error> {
	let Some(cache) = cache.filter(|_| level != NO_COMPRESSION_LEVEL) else {
		return codec::deflate(raw, level);
	};
	let key = CacheKey::for_chunk(raw, level);
	if let Some(bytes) = cache.lookup(key, raw) {
		return Ok(Some(bytes));
	}
	let encoded = codec::deflate(raw, level)?;
	if let Some(bytes) = &encoded {
		cache.backend.store(key, bytes);
	}
	Ok(encoded)
}

pub(super) fn scratch_bytes(options: &WriteOptions) -> usize {
	if options.zlib_level == NO_COMPRESSION_LEVEL {
		0
	} else {
		codec::CACHE_INFLATE_SCRATCH_BYTES
	}
}
