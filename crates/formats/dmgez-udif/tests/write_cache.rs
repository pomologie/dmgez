#![cfg(feature = "write-cache")]

use std::{collections::HashMap, io, num::NonZeroUsize, sync::Mutex};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	writer::{
		Profile, WriteOptions,
		cache::{CacheKey, CacheStats, ChunkCache},
		write_image, write_image_with_cache,
	},
};
use sha2::{Digest, Sha256};

const CHUNK_BYTES: usize = 128 * SECTOR_BYTES as usize;
const FIRST_BYTE: u8 = b'a';
const SECOND_BYTE: u8 = b'b';
const THIRD_BYTE: u8 = b'c';
const DIFFERENT_LEVEL: u8 = 1;
const NO_COMPRESSION: u8 = 0;
const QUALIFIED_ENCODED_DIGEST: [u8; 32] = [
	0x4a, 0x2e, 0x44, 0x0c, 0x40, 0xb3, 0xa8, 0xa3, 0xf0, 0x28, 0xdc, 0xd9, 0x0d, 0xf8, 0xdf, 0xa0,
	0x3c, 0x79, 0xbf, 0xd4, 0x1f, 0x8e, 0x32, 0x0b, 0xe2, 0x92, 0xb4, 0xa0, 0xb6, 0x37, 0x66, 0xef,
];

#[derive(Default)]
struct MemoryCache(Mutex<HashMap<CacheKey, Vec<u8>>>);

impl ChunkCache for MemoryCache {
	fn load(&self, key: CacheKey, output: &mut [u8]) -> Option<usize> {
		let entries = self.0.lock().unwrap();
		let encoded = entries.get(&key)?;
		if let Some(output) = output.get_mut(..encoded.len()) {
			output.copy_from_slice(encoded);
		}
		Some(encoded.len())
	}

	fn store(&self, key: CacheKey, encoded: &[u8]) {
		self.0.lock().unwrap().insert(key, encoded.to_vec());
	}
}

fn options() -> WriteOptions {
	WriteOptions {
		chunk_bytes: CHUNK_BYTES,
		..WriteOptions::default()
	}
}

fn source(chunks: &[u8]) -> Vec<u8> {
	chunks
		.iter()
		.flat_map(|&value| std::iter::repeat_n(value, CHUNK_BYTES))
		.collect()
}

fn cached(logical: &[u8], options: &WriteOptions, cache: &MemoryCache) -> (Vec<u8>, CacheStats) {
	let mut image = Vec::new();
	let (_, stats) =
		write_image_with_cache(logical, logical.len() as u64, &mut image, options, cache).unwrap();
	(image, stats)
}

fn uncached(logical: &[u8], options: &WriteOptions) -> Vec<u8> {
	let mut image = Vec::new();
	write_image(logical, logical.len() as u64, &mut image, options).unwrap();
	image
}

fn assert_stats(stats: CacheStats, hits: usize, misses: usize, rejected: usize) {
	assert_eq!(
		(stats.hits, stats.misses, stats.rejected),
		(hits, misses, rejected)
	);
}

#[test]
fn qualified_encoder_matches_its_reference_stream() {
	const FIXTURE_PERIOD: usize = 251;
	const FIXTURE_SHIFT: usize = 4;
	let raw: Vec<u8> = (0..CHUNK_BYTES)
		.map(|offset| ((offset >> FIXTURE_SHIFT) ^ (offset % FIXTURE_PERIOD)) as u8)
		.collect();
	let cache = MemoryCache::default();
	cached(&raw, &options(), &cache);
	let entries = cache.0.into_inner().unwrap();
	let candidate = entries.values().next().unwrap();
	// A changed codec output needs a newly qualified cache revision.
	assert_eq!(
		Sha256::digest(candidate).as_slice(),
		QUALIFIED_ENCODED_DIGEST
	);
}

#[test]
fn reuses_bytes_at_new_offsets_and_keys_compression_options() {
	let mut options = options();
	let cache = MemoryCache::default();
	let logical = source(&[FIRST_BYTE, SECOND_BYTE]);
	let (cold, stats) = cached(&logical, &options, &cache);
	assert_stats(stats, 0, 2, 0);
	assert_eq!(cold, uncached(&logical, &options));
	let relocated = source(&[SECOND_BYTE, FIRST_BYTE]);
	let (warm, stats) = cached(&relocated, &options, &cache);
	assert_stats(stats, 2, 0, 0);
	assert_eq!(warm, uncached(&relocated, &options));

	let edited = source(&[SECOND_BYTE, THIRD_BYTE]);
	let (partial, stats) = cached(&edited, &options, &cache);
	assert_stats(stats, 1, 1, 0);
	assert_eq!(partial, uncached(&edited, &options));

	options.zlib_level = DIFFERENT_LEVEL;
	let (different_level, stats) = cached(&edited, &options, &cache);
	assert_stats(stats, 0, 2, 0);
	assert_eq!(different_level, uncached(&edited, &options));
}

#[test]
fn rejects_malformed_oversized_and_wrong_content_and_regenerates() {
	let options = options();
	let cache = MemoryCache::default();
	let logical = source(&[FIRST_BYTE]);
	let (correct_image, _) = cached(&logical, &options, &cache);
	let (key, canonical) = {
		let entries = cache.0.lock().unwrap();
		let (key, encoded) = entries.iter().next().unwrap();
		(*key, encoded.clone())
	};
	let wrong_cache = MemoryCache::default();
	cached(&source(&[SECOND_BYTE]), &options, &wrong_cache);
	let wrong = wrong_cache
		.0
		.into_inner()
		.unwrap()
		.into_values()
		.next()
		.unwrap();
	let short_cache = MemoryCache::default();
	let short_source = vec![FIRST_BYTE; CHUNK_BYTES - SECTOR_BYTES as usize];
	cached(&short_source, &options, &short_cache);
	let short = short_cache
		.0
		.into_inner()
		.unwrap()
		.into_values()
		.next()
		.unwrap();
	let long_cache = MemoryCache::default();
	let long_options = WriteOptions {
		chunk_bytes: 2 * CHUNK_BYTES,
		..options.clone()
	};
	cached(
		&source(&[FIRST_BYTE, FIRST_BYTE]),
		&long_options,
		&long_cache,
	);
	let long = long_cache
		.0
		.into_inner()
		.unwrap()
		.into_values()
		.next()
		.unwrap();
	let mut trailed = canonical.clone();
	trailed.push(0);
	let truncated = canonical[..canonical.len() - 1].to_vec();
	let oversized = vec![0; CHUNK_BYTES];
	for poisoned in [wrong, short, long, trailed, truncated, oversized] {
		cache.0.lock().unwrap().insert(key, poisoned);
		let (rebuilt, stats) = cached(&logical, &options, &cache);
		assert_stats(stats, 0, 1, 1);
		assert_eq!(rebuilt, correct_image);
		assert_eq!(cache.0.lock().unwrap()[&key], canonical);
	}
}

#[test]
fn keeps_zeros_and_level_zero_outside_the_compressed_cache() {
	let mut options = options();
	let cache = MemoryCache::default();
	let zero = source(&[0]);
	let (image, stats) = cached(&zero, &options, &cache);
	assert_eq!(stats, CacheStats::default());
	assert_eq!(image, uncached(&zero, &options));

	options.zlib_level = NO_COMPRESSION;
	let raw = source(&[FIRST_BYTE]);
	let (image, stats) = cached(&raw, &options, &cache);
	assert_eq!(stats, CacheStats::default());
	assert_eq!(image, uncached(&raw, &options));
	assert!(cache.0.lock().unwrap().is_empty());
}

#[test]
fn admits_decoder_scratch_before_reading() {
	let mut options = options();
	let normal = options
		.admit(NonZeroUsize::MIN)
		.unwrap()
		.accounted_payload_bytes();
	options.max_buffer_bytes = normal;
	assert!(matches!(
		write_image_with_cache(
			io::empty(),
			SECTOR_BYTES,
			io::sink(),
			&options,
			&MemoryCache::default()
		),
		Err(Error::ResourceLimit(_))
	));
	options.profile = Profile::Udro;
	assert!(matches!(
		options.admit_with_cache(NonZeroUsize::MIN),
		Err(Error::Unsupported(_))
	));
}

#[cfg(feature = "parallel")]
#[test]
fn warm_cache_preserves_order_with_parallel_encoding() {
	use dmgez_udif::writer::Execution;

	let cache = MemoryCache::default();
	let mut options = options();
	let first = source(&[FIRST_BYTE, SECOND_BYTE, THIRD_BYTE]);
	cached(&first, &options, &cache);
	let logical = source(&[THIRD_BYTE, FIRST_BYTE, SECOND_BYTE, FIRST_BYTE]);
	let expected = uncached(&logical, &options);
	options.execution = Execution::Parallel { workers: 2 };
	let (reused, stats) = cached(&logical, &options, &cache);
	assert_stats(stats, 4, 0, 0);
	assert_eq!(reused, expected);
}
