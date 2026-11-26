#[cfg(dmgez_loom)]
use loom::{
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	thread,
};
#[cfg(not(dmgez_loom))]
use std::{
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	thread,
};

use crate::{
	Error, SECTOR_BYTES,
	geometry::DECODER_SPARE_BYTES,
	reader::{Image, Limits, ReadAt},
	writer::{Profile, WriteOptions, write_image},
};

const RUN_BYTES: usize = SECTOR_BYTES as usize;
const TWO_RUNS: usize = 2 * (RUN_BYTES + DECODER_SPARE_BYTES);
const CODEC_TEST_STACK_BYTES: usize = 2 << 20;

struct ObservedSource {
	image: Vec<u8>,
	reads: Arc<AtomicUsize>,
}

impl ReadAt for ObservedSource {
	fn len(&self) -> u64 {
		self.image.len() as u64
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		self.reads.fetch_add(1, Ordering::SeqCst);
		self.image.as_slice().read_exact_at(offset, output)
	}
}

fn spawn(task: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
	thread::Builder::new()
		.stack_size(CODEC_TEST_STACK_BYTES)
		.spawn(task)
		.unwrap()
}

fn model(run: impl Fn() + Send + Sync + Copy + 'static) {
	#[cfg(dmgez_loom)]
	loom::model(move || spawn(run).join().unwrap());
	#[cfg(not(dmgez_loom))]
	run();
}

fn fixture(values: [u8; 3], capacity: usize) -> (Image<ObservedSource>, Vec<u8>, Arc<AtomicUsize>) {
	let raw: Vec<_> = values
		.into_iter()
		.flat_map(|byte| std::iter::repeat_n(byte, RUN_BYTES))
		.collect();
	let mut image = Vec::new();
	let written = write_image(
		raw.as_slice(),
		raw.len() as u64,
		&mut image,
		&WriteOptions {
			profile: Profile::Udzo,
			chunk_bytes: RUN_BYTES,
			..WriteOptions::default()
		},
	)
	.unwrap();
	assert_eq!(written.compressed_runs, values.len());
	let reads = Arc::new(AtomicUsize::new(0));
	let image = Image::open(
		ObservedSource {
			image,
			reads: Arc::clone(&reads),
		},
		Limits {
			max_decoded_bytes: capacity - DECODER_SPARE_BYTES,
			..Limits::default()
		},
	)
	.unwrap();
	reads.store(0, Ordering::SeqCst);
	(image, raw, reads)
}

fn check_range(image: &Image<ObservedSource>, expected: &[u8], start: usize, count: usize) {
	let mut output = vec![0; count];
	image.read_exact_at(start as u64, &mut output).unwrap();
	assert_eq!(output, expected[start..start + count]);
}

#[test]
fn overlapping_cross_run_reads_decode_each_run_once() {
	model(|| {
		let (image, raw, read_count) = fixture([1, 2, 3], TWO_RUNS);
		let image = Arc::new(image);
		let other_image = Arc::clone(&image);
		let other_raw = raw.clone();
		let thread = spawn(move || {
			check_range(&other_image, &other_raw, RUN_BYTES - 4, 18);
		});
		check_range(&image, &raw, RUN_BYTES - 6, 18);
		thread.join().unwrap();
		assert_eq!(read_count.load(Ordering::SeqCst), 2);
	});
}

#[test]
fn overlapping_reads_remain_exact_during_eviction() {
	model(|| {
		let (image, raw, reads) = fixture([1, 2, 3], TWO_RUNS);
		let image = Arc::new(image);
		let other_image = Arc::clone(&image);
		let other_raw = raw.clone();
		let thread = spawn(move || {
			check_range(
				&other_image,
				&other_raw,
				RUN_BYTES + RUN_BYTES / 2,
				RUN_BYTES,
			);
		});
		check_range(&image, &raw, RUN_BYTES / 2, RUN_BYTES + RUN_BYTES / 4);
		thread.join().unwrap();
		// An interleaving can evict the shared run before the second reader reaches it.
		assert!((3..=4).contains(&reads.load(Ordering::SeqCst)));
	});
}

#[test]
fn evicts_oldest_run_and_charges_retained_decoder_capacity() {
	model(|| {
		let (image, raw, reads) = fixture([1, 2, 3], TWO_RUNS);
		// Touch the first run before the third arrives, so the second is evicted.
		for run in [0, 1, 0, 2, 0] {
			check_range(&image, &raw, run * RUN_BYTES, 1);
		}
		assert_eq!(reads.load(Ordering::SeqCst), 3);
		check_range(&image, &raw, RUN_BYTES, 1);
		assert_eq!(reads.load(Ordering::SeqCst), 4);

		let (image, raw, reads) = fixture([1, 2, 3], TWO_RUNS - 1);
		for run in [0, 1, 0] {
			check_range(&image, &raw, run * RUN_BYTES, 1);
		}
		assert_eq!(reads.load(Ordering::SeqCst), 3);
	});
}

#[test]
fn equal_segment_ids_do_not_share_cached_bytes() {
	model(|| {
		let (left, left_raw, left_reads) = fixture([1, 2, 3], TWO_RUNS);
		let (right, right_raw, right_reads) = fixture([4, 5, 6], TWO_RUNS);
		assert_eq!(left.descriptor().segment_id, right.descriptor().segment_id);
		assert_ne!(left_raw, right_raw);
		let thread = spawn(move || {
			check_range(&right, &right_raw, RUN_BYTES - 4, 18);
		});
		check_range(&left, &left_raw, RUN_BYTES - 4, 18);
		thread.join().unwrap();
		assert_eq!(left_reads.load(Ordering::SeqCst), 2);
		assert_eq!(right_reads.load(Ordering::SeqCst), 2);
	});
}
