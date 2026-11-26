#![cfg(feature = "write")]

use std::{
	io::{self, Read, Write},
	num::NonZeroUsize,
	ops::Range,
};

use dmgez_udif::{
	Error, SECTOR_BYTES,
	writer::{Profile, WriteOptions, write_image},
};

const SECTOR_SIZE: usize = SECTOR_BYTES as usize;
const FRAGMENT_BYTES: usize = 37;

struct FragmentedSource<'data>(&'data [u8]);

impl Read for FragmentedSource<'_> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let size = buffer.len().min(FRAGMENT_BYTES);
		self.0.read(&mut buffer[..size])
	}
}

struct RecordingSink {
	bytes: Vec<u8>,
	writes: Vec<Range<usize>>,
	limit: usize,
}

impl RecordingSink {
	fn new(limit: usize) -> Self {
		Self {
			bytes: Vec::new(),
			writes: Vec::new(),
			limit,
		}
	}
}

impl Write for RecordingSink {
	fn write(&mut self, data: &[u8]) -> io::Result<usize> {
		let start = self.bytes.len();
		let count = data.len().min(FRAGMENT_BYTES).min(self.limit - start);
		let end = start + count;
		self.bytes.extend_from_slice(&data[..count]);
		self.writes.push(start..end);
		Ok(count)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[cfg(all(feature = "read", feature = "zlib"))]
#[test]
fn nonseekable_profiles_preserve_bytes_offsets_and_raw_fallback() {
	use dmgez_udif::reader::{Image, Limits};

	// Two nonzero sectors. The repeated bytes admit zlib, while the xorshift
	// bytes require a raw run with the pinned encoder.
	let mut logical = [b'A'; 2 * SECTOR_SIZE];
	let mut state = 0x9e37_79b9_u32;
	for byte in &mut logical[SECTOR_SIZE..] {
		state ^= state << 13;
		state ^= state >> 17;
		state ^= state << 5;
		*byte = state.to_be_bytes()[0];
	}

	for (profile, raw_runs, compressed_runs) in [(Profile::Udro, 2, 0), (Profile::Udzo, 1, 1)] {
		let options = WriteOptions {
			profile,
			chunk_bytes: SECTOR_SIZE,
			..WriteOptions::default()
		};
		let mut sink = RecordingSink::new(usize::MAX);
		let report = write_image(
			FragmentedSource(&logical),
			logical.len() as u64,
			&mut sink,
			&options,
		)
		.unwrap();
		let mut end = 0;
		for write in &sink.writes {
			assert_eq!(write.start, end);
			assert!((1..=FRAGMENT_BYTES).contains(&write.len()));
			end = write.end;
		}
		assert_eq!(end, sink.bytes.len());
		assert_eq!(report.physical_bytes, end as u64);
		assert_eq!(report.raw_runs, raw_runs);
		assert_eq!(report.compressed_runs, compressed_runs);

		let mut image = Image::open(sink.bytes.as_slice(), Limits::default()).unwrap();
		image.verify().unwrap();
		let mut decoded = Vec::new();
		image.write_raw_to(&mut decoded).unwrap();
		assert_eq!(decoded, logical);
	}
}

#[test]
fn source_length_and_sink_failures_leave_only_an_unfinished_prefix() {
	let logical = [b'A'; 3 * SECTOR_SIZE];
	let expected = 2 * SECTOR_BYTES;
	let options = WriteOptions {
		profile: Profile::Udro,
		chunk_bytes: SECTOR_SIZE,
		..WriteOptions::default()
	};
	for (length, expected_prefix) in [(SECTOR_SIZE, SECTOR_SIZE), (logical.len(), 2 * SECTOR_SIZE)]
	{
		let mut sink = RecordingSink::new(usize::MAX);
		assert!(matches!(
			write_image(
				FragmentedSource(&logical[..length]),
				expected,
				&mut sink,
				&options
			),
			Err(Error::SourceLength(_))
		));
		assert_eq!(sink.bytes, logical[..expected_prefix]);
	}
	let fail_at = SECTOR_SIZE + FRAGMENT_BYTES;
	let mut sink = RecordingSink::new(fail_at);
	assert!(matches!(
		write_image(
			FragmentedSource(&logical[..2 * SECTOR_SIZE]),
			expected,
			&mut sink,
			&options
		),
		Err(Error::Io(error)) if error.kind() == io::ErrorKind::WriteZero
	));
	assert_eq!(sink.bytes, logical[..fail_at]);
}

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
