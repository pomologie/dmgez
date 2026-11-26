#![cfg(feature = "write")]

use std::{
	io::{self, Read, Write},
	num::NonZeroUsize,
};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	writer::{Profile, WriteOptions, write_image},
};

const SECTOR_SIZE: usize = SECTOR_BYTES as usize;

#[test]
fn rejects_unaligned_short_long_and_over_budget_input() {
	let short = [0; SECTOR_SIZE - 1];
	let aligned = [0; SECTOR_SIZE];
	let long = [0; SECTOR_SIZE + 1];
	let options = WriteOptions {
		profile: Profile::Udro,
		..WriteOptions::default()
	};
	assert!(matches!(
		write_image(&short[..], SECTOR_BYTES - 1, io::sink(), &options),
		Err(Error::UnalignedSource)
	));
	assert!(matches!(
		write_image(&short[..], SECTOR_BYTES, io::sink(), &options),
		Err(Error::SourceLength(_))
	));
	assert!(matches!(
		write_image(&long[..], SECTOR_BYTES, io::sink(), &options),
		Err(Error::SourceLength(_))
	));
	let budget = WriteOptions {
		max_buffer_bytes: SECTOR_SIZE - 1,
		..options.clone()
	};
	assert!(matches!(
		write_image(&aligned[..], SECTOR_BYTES, io::sink(), &budget),
		Err(Error::ResourceLimit(_))
	));
	let physical = WriteOptions {
		max_physical_bytes: 900,
		..options
	};
	assert!(matches!(
		write_image(&aligned[..], SECTOR_BYTES, io::sink(), &physical),
		Err(Error::ResourceLimit(_))
	));
}

#[test]
fn validates_the_same_payload_budget_before_streaming() {
	struct Unopened;
	impl Read for Unopened {
		fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
			panic!("rejected writer must not read source bytes");
		}
	}

	let mut options = WriteOptions {
		profile: Profile::Udro,
		chunk_bytes: SECTOR_SIZE,
		max_buffer_bytes: SECTOR_SIZE - 1,
		..WriteOptions::default()
	};
	let Err(Error::ResourceLimit(expected)) = options.admit(NonZeroUsize::MIN) else {
		panic!("pure admission must reject insufficient payload memory");
	};
	assert!(matches!(
		write_image(Unopened, SECTOR_BYTES, io::sink(), &options),
		Err(Error::ResourceLimit(actual)) if actual == expected
	));

	options.max_buffer_bytes = SECTOR_SIZE;
	let logical = [0; 2 * SECTOR_SIZE];
	let logical_bytes = logical.len() as u64;
	let admitted = options.admit(NonZeroUsize::MIN).unwrap();
	let report = write_image(logical.as_slice(), logical_bytes, io::sink(), &options).unwrap();
	assert_eq!(
		report.raw_runs,
		options.max_data_runs(logical_bytes).unwrap()
	);
	assert_eq!(
		report.accounted_payload_bytes,
		admitted.accounted_payload_bytes()
	);
}

#[test]
fn aborts_a_cancelled_or_failing_nonseekable_sink() {
	struct FailedSink(usize);
	impl Write for FailedSink {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			if self.0 == 0 {
				return Err(io::Error::other("injected failure"));
			}
			let len = self.0.min(bytes.len());
			self.0 -= len;
			Ok(len)
		}
		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}
	let options = WriteOptions {
		profile: Profile::Udro,
		..WriteOptions::default()
	};
	let sector = [2; SECTOR_SIZE];
	assert!(matches!(
		write_image(&sector[..], SECTOR_BYTES, FailedSink(800), &options),
		Err(Error::Io(_))
	));
	options.cancellation.cancel();
	assert!(matches!(
		write_image(&sector[..], SECTOR_BYTES, io::sink(), &options),
		Err(Error::Cancelled)
	));
}

#[cfg(all(feature = "read", feature = "zlib", feature = "parallel"))]
#[test]
fn parallel_output_is_identical_at_different_worker_counts() {
	use dmgez_udif::{
		reader::{Image, Limits},
		writer::Execution,
	};

	let mut logical = vec![0; 8 * SECTOR_SIZE];
	logical[2 * SECTOR_SIZE..4 * SECTOR_SIZE].fill(17);
	logical[6 * SECTOR_SIZE..].fill(91);
	let mut reference = Vec::new();
	let options = WriteOptions {
		chunk_bytes: SECTOR_SIZE,
		..WriteOptions::default()
	};
	let report = write_image(
		logical.as_slice(),
		logical.len() as u64,
		&mut reference,
		&options,
	)
	.unwrap();
	assert_eq!(report.zero_runs, 2);
	assert_eq!(report.compressed_runs, 4);
	for workers in [0, 2, 4] {
		let options = WriteOptions {
			execution: Execution::Parallel { workers },
			..options.clone()
		};
		let mut output = Vec::new();
		let report = write_image(
			logical.as_slice(),
			logical.len() as u64,
			&mut output,
			&options,
		)
		.unwrap();
		assert_eq!(output, reference);
		assert!(report.accounted_payload_bytes <= options.max_buffer_bytes);
	}
	let mut image = Image::open(reference.as_slice(), Limits::default()).unwrap();
	image.verify().unwrap();
	let start = 2 * SECTOR_SIZE - 1;
	let end = 6 * SECTOR_SIZE + 1;
	let mut window = vec![u8::MAX; end - start];
	image.read_exact_at(start as u64, &mut window).unwrap();
	assert_eq!(window, logical[start..end]);
	let mut decoded = Vec::new();
	image.write_raw_to(&mut decoded).unwrap();
	assert_eq!(decoded, logical);
}
