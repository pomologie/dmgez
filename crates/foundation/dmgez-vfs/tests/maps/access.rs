//! Exact selected-window export and failure cases for D-READ-01-T01 and D-V09.

use std::{
	cell::{Cell, RefCell},
	io::{self, Write},
};

use dmgez_udif::{Error as SourceError, reader::ReadAt};

use super::*;

const PARTITION_FIRST_SECTOR: u32 = 3;
const PARTITION_SECTORS: u32 = 2;
const PARTITION_SLOT: u32 = 2;
const PAYLOAD_BYTE: u8 = 0xCD;
const DISK_BYTES: usize = 12 * SECTOR;
const RAW_START: usize = PARTITION_FIRST_SECTOR as usize * SECTOR;
const RAW_LEN: usize = PARTITION_SECTORS as usize * SECTOR;
const SCRATCH_BYTES: usize = 127;

fn fixture() -> [u8; DISK_BYTES] {
	let mut raw = [0x31; DISK_BYTES];
	let mut boot = EMPTY;
	put_entry(
		&mut boot,
		PARTITION_SLOT as usize - 1,
		APPLE_HFS,
		PARTITION_FIRST_SECTOR,
		PARTITION_SECTORS,
	);
	raw[..SECTOR].copy_from_slice(&boot);
	raw[RAW_START..RAW_START + RAW_LEN].fill(PAYLOAD_BYTE);
	raw
}

struct ObservedSource<'fixture> {
	bytes: &'fixture [u8],
	changed: &'fixture Cell<bool>,
	reads: &'fixture RefCell<Vec<(u64, usize)>>,
}

impl ReadAt for ObservedSource<'_> {
	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn check_unchanged(&self) -> Result<(), SourceError> {
		if self.changed.get() {
			return Err(SourceError::SourceChanged);
		}
		Ok(())
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		self.reads.borrow_mut().push((offset, output.len()));
		self.bytes.read_exact_at(offset, output)
	}
}

#[test]
fn streamed_subwindow_stays_in_absolute_bounds_and_revalidates_at_eof() {
	const SUBOFFSET: usize = 3;
	const SUBLEN: usize = SECTOR + 1;
	let raw = fixture();
	let changed = Cell::new(false);
	let reads = RefCell::new(Vec::new());
	let source = ObservedSource {
		bytes: &raw,
		changed: &changed,
		reads: &reads,
	};
	let mut disk = Disk::discover(Raw(source), Limits::default()).unwrap();
	reads.borrow_mut().clear();
	let id = disk.select_volume(None).unwrap();
	let mut window = disk.data_window(id).unwrap();
	let mut subwindow = window.subwindow(SUBOFFSET as u64, SUBLEN as u64).unwrap();
	let mut output = Vec::new();
	subwindow
		.write_raw_to(&mut output, &mut [0; SCRATCH_BYTES], || false)
		.unwrap();
	let start = RAW_START + SUBOFFSET;
	assert_eq!(subwindow.logical_offset(), start as u64);
	assert_eq!(output, raw[start..start + SUBLEN]);
	let mut expected: Vec<_> = (0..SUBLEN)
		.step_by(SCRATCH_BYTES)
		.map(|offset| {
			(
				(start + offset) as u64,
				(SUBLEN - offset).min(SCRATCH_BYTES),
			)
		})
		.collect();
	expected.push(((start + SUBLEN) as u64, 0));
	assert_eq!(*reads.borrow(), expected);
}

#[test]
fn stream_errors_distinguish_buffer_policy_cancellation_and_sink_failure() {
	let raw = fixture();
	let mut disk = Disk::discover(Raw(raw.as_slice()), Limits::default()).unwrap();
	let id = disk.select_volume(None).unwrap();
	let mut window = disk.window(id).unwrap();
	let mut output = Vec::new();
	assert!(matches!(
		window.write_raw_to(&mut output, &mut [], || false),
		Err(Error::EmptyCopyBuffer)
	));
	assert!(output.is_empty());

	let mut polls = 0;
	let result = window.write_raw_to(&mut output, &mut [0; SCRATCH_BYTES], || {
		polls += 1;
		polls == 2
	});
	assert!(matches!(result, Err(Error::Cancelled)));
	assert_eq!(output, raw[RAW_START..RAW_START + SCRATCH_BYTES]);

	const WRITABLE_PREFIX: usize = 3;
	struct FailingSink(usize);
	impl Write for FailingSink {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			let written = (WRITABLE_PREFIX - self.0).min(bytes.len());
			if written == 0 {
				return Err(io::ErrorKind::BrokenPipe.into());
			}
			self.0 += written;
			Ok(written)
		}

		fn flush(&mut self) -> io::Result<()> {
			panic!("a failed sink cannot be flushed by raw export");
		}
	}
	let mut sink = FailingSink(0);
	let result = window.write_raw_to(&mut sink, &mut [0; SCRATCH_BYTES], || false);
	assert!(matches!(result, Err(Error::Sink(cause)) if cause.kind() == io::ErrorKind::BrokenPipe));
	assert_eq!(sink.0, WRITABLE_PREFIX);
}

#[test]
fn source_replacement_after_the_final_write_prevents_success() {
	struct ReplacingSink<'fixture> {
		changed: &'fixture Cell<bool>,
		written: usize,
	}
	impl Write for ReplacingSink<'_> {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			self.written += bytes.len();
			if self.written == RAW_LEN {
				self.changed.set(true);
			}
			Ok(bytes.len())
		}

		fn flush(&mut self) -> io::Result<()> {
			panic!("a changed source cannot finish raw export");
		}
	}

	let raw = fixture();
	let changed = Cell::new(false);
	let reads = RefCell::new(Vec::new());
	let source = ObservedSource {
		bytes: &raw,
		changed: &changed,
		reads: &reads,
	};
	let mut disk = Disk::discover(Raw(source), Limits::default()).unwrap();
	let id = disk.select_volume(None).unwrap();
	let mut window = disk.window(id).unwrap();
	let mut sink = ReplacingSink {
		changed: &changed,
		written: 0,
	};
	assert!(matches!(
		window.write_raw_to(&mut sink, &mut [0; SCRATCH_BYTES], || false),
		Err(Error::Source(SourceError::SourceChanged))
	));
	assert_eq!(sink.written, RAW_LEN);
}

#[cfg(feature = "zlib")]
#[test]
fn native_apm_partition_export_matches_apple_raw_export() {
	use dmgez_udif::reader::{Image, Limits as UdifLimits};

	const NATIVE_APM: &[u8] = include_bytes!("../fixtures/native-apm.dmg");
	const EXPECTED_START: u64 = 32_768;
	const EXPECTED_LENGTH: u64 = 16_744_448;
	const EXPECTED_CRC32: u32 = 0xF09E_E540;
	const NON_SECTOR_BUFFER_BYTES: usize = 4_093;

	struct DigestSink(crc32fast::Hasher, u64);
	impl Write for DigestSink {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			self.0.update(bytes);
			self.1 += bytes.len() as u64;
			Ok(bytes.len())
		}

		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}

	let image = Image::open(NATIVE_APM, UdifLimits::default()).unwrap();
	let mut disk = Disk::discover(image, Limits::default()).unwrap();
	let id = disk.select_volume(None).unwrap();
	assert_eq!(id, selector(Scheme::Apple, 2));
	let mut window = disk.window(id).unwrap();
	assert_eq!(window.logical_offset(), EXPECTED_START);
	let mut sink = DigestSink(crc32fast::Hasher::new(), 0);
	window
		.write_raw_to(&mut sink, &mut [0; NON_SECTOR_BUFFER_BYTES], || false)
		.unwrap();
	assert_eq!(sink.1, EXPECTED_LENGTH);
	assert_eq!(sink.0.finalize(), EXPECTED_CRC32);
}
