//! Streaming UDRO and UDZO construction from finite logical disk bytes.

mod chunk;
mod scheduler;

use std::{
	io::{self, Read, Write},
	num::NonZeroUsize,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
};

use crc32fast::Hasher;

use crate::{
	Error, SECTOR_BYTES,
	blkx::{self, Kind, Record},
	checksum, resource, trailer,
};
use scheduler::Scheduler;

const MAX_CHUNK_BYTES: usize = 1 << 20;
const MAX_RECORDS: usize = 131_072;
const DEFAULT_ZLIB_LEVEL: u8 = 6;
const MAX_ZLIB_LEVEL: u8 = 9;
const DEFAULT_IMAGE_SIZE_LIMIT_BYTES: u64 = 1 << 40;
const DEFAULT_PAYLOAD_BUDGET_BYTES: usize = 32 << 20;
const TERMINATOR_RECORD_COUNT: usize = 1;
const RAW_ONLY_BUFFER_SECTORS: u32 = 0;
// Native 1 MiB zlib chunks declare 2056 buffer sectors for 2048 decoded sectors.
const ZLIB_BUFFER_HEADROOM_SECTORS: u32 = 8;

/// Output codec profile for the single-table partition image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
	/// Uncompressed raw data runs.
	Udro,
	/// Zlib runs, explicit zeros and raw fallback for incompressible data.
	Udzo,
	/// LZFSE runs, explicit zeros and raw fallback for incompressible data.
	Ulfo,
	/// LZMA2 in XZ runs, explicit zeros and raw fallback for incompressible data.
	Ulmo,
}

/// Per-write compression scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Execution {
	/// Encode in the caller thread.
	Serial,
	/// Encode bounded batches using a private Rayon pool when multiple workers are selected.
	Parallel {
		/// Maximum workers. Zero automatically selects a host- and budget-aware count.
		workers: usize,
	},
}

impl Execution {
	/// Select a host- and budget-aware worker count for [`Execution::Parallel`].
	pub const AUTO_WORKERS: usize = 0;
}

/// A shared flag for cooperative cancellation at chunk and output boundaries.
#[derive(Debug, Default, Clone)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
	/// Request termination of an in-progress write.
	pub fn cancel(&self) {
		self.0.store(true, Ordering::Relaxed);
	}

	/// Check whether termination was requested.
	#[must_use]
	pub fn is_cancelled(&self) -> bool {
		self.0.load(Ordering::Relaxed)
	}

	fn check(&self) -> Result<(), Error> {
		if self.is_cancelled() {
			Err(Error::Cancelled)
		} else {
			Ok(())
		}
	}
}

/// Resource limits and deterministic encoding options.
#[derive(Debug, Clone)]
pub struct WriteOptions {
	/// Container codec profile.
	pub profile: Profile,
	/// Chunk size in bytes, from 512 bytes to 1 MiB in whole sectors.
	pub chunk_bytes: usize,
	/// Zlib compression level from 0 through 9.
	pub zlib_level: u8,
	/// Maximum admitted logical disk size in bytes.
	pub max_logical_bytes: u64,
	/// Maximum total output size in bytes, including metadata.
	pub max_physical_bytes: u64,
	/// Budget for in-flight payload buffers and accounted codec scratch.
	pub max_buffer_bytes: usize,
	/// Compression scheduling. Either mode emits the same bytes for identical settings.
	pub execution: Execution,
	/// The caller can clone this token before starting a write.
	pub cancellation: Cancellation,
}

impl Default for WriteOptions {
	fn default() -> Self {
		Self {
			profile: Profile::Udzo,
			chunk_bytes: MAX_CHUNK_BYTES,
			zlib_level: DEFAULT_ZLIB_LEVEL,
			max_logical_bytes: DEFAULT_IMAGE_SIZE_LIMIT_BYTES,
			max_physical_bytes: DEFAULT_IMAGE_SIZE_LIMIT_BYTES,
			max_buffer_bytes: DEFAULT_PAYLOAD_BUDGET_BYTES,
			execution: Execution::Serial,
			cancellation: Cancellation::default(),
		}
	}
}

/// Resolved worker width and peak charged payload allocations for one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteAdmission {
	workers: NonZeroUsize,
	accounted_payload_bytes: usize,
}

impl WriteAdmission {
	/// Returns serial execution or a fixed parallel width already admitted by the budget.
	#[must_use]
	pub const fn execution(self) -> Execution {
		if self.workers.get() == NonZeroUsize::MIN.get() {
			Execution::Serial
		} else {
			Execution::Parallel {
				workers: self.workers.get(),
			}
		}
	}

	/// Returns the admitted peak in-flight payload and accounted codec scratch.
	#[must_use]
	pub const fn accounted_payload_bytes(self) -> usize {
		self.accounted_payload_bytes
	}
}

impl WriteOptions {
	/// Maximum number of data runs under the current single-table profile.
	pub const MAX_DATA_RUNS: usize = MAX_RECORDS - TERMINATOR_RECORD_COUNT;

	/// Validates encoder settings and resolves worker width without allocating or reading.
	/// An explicit worker count is a cap chosen by the caller. The supplied
	/// host width is used only by automatic parallel scheduling.
	///
	/// # Errors
	///
	/// Rejects unavailable codec or scheduling features, invalid chunk size or
	/// level, and budgets insufficient for the requested scheduling.
	pub fn admit(&self, available_workers: NonZeroUsize) -> Result<WriteAdmission, Error> {
		self.validate_chunk_bytes()?;
		if self.zlib_level > MAX_ZLIB_LEVEL {
			return Err(Error::ResourceLimit("zlib level must be between 0 and 9"));
		}
		if self.profile == Profile::Udzo && !cfg!(feature = "zlib") {
			return Err(Error::FeatureNotCompiled("zlib"));
		}
		if self.profile == Profile::Ulfo && !cfg!(feature = "lzfse") {
			return Err(Error::FeatureNotCompiled("lzfse"));
		}
		if self.profile == Profile::Ulmo && !cfg!(feature = "lzma") {
			return Err(Error::FeatureNotCompiled("lzma"));
		}
		if self.max_physical_bytes <= trailer::BYTES as u64 {
			return Err(Error::ResourceLimit("physical image size"));
		}
		scheduler::admit(self, available_workers)
	}

	/// Returns the worst-case data run count for a finite logical disk.
	/// Adjacent zero chunks may reduce the emitted count.
	///
	/// # Errors
	///
	/// Rejects invalid geometry, chunk settings, or sizes exceeding the
	/// logical size or single-table record limits.
	pub fn max_data_runs(&self, logical_bytes: u64) -> Result<usize, Error> {
		if logical_bytes == 0 || !logical_bytes.is_multiple_of(SECTOR_BYTES) {
			return Err(Error::UnalignedSource);
		}
		if logical_bytes > self.max_logical_bytes {
			return Err(Error::ResourceLimit("logical disk size"));
		}
		self.validate_chunk_bytes()?;
		let runs = logical_bytes.div_ceil(self.chunk_bytes as u64);
		if runs > Self::MAX_DATA_RUNS as u64 {
			return Err(Error::ResourceLimit("maximum block-map records"));
		}
		Ok(runs as usize)
	}

	fn validate_chunk_bytes(&self) -> Result<(), Error> {
		if !(SECTOR_BYTES as usize..=MAX_CHUNK_BYTES).contains(&self.chunk_bytes)
			|| !self.chunk_bytes.is_multiple_of(SECTOR_BYTES as usize)
		{
			return Err(Error::ResourceLimit(
				"chunk size must be sector-aligned and at most 1 MiB",
			));
		}
		Ok(())
	}
}

/// Geometry and checksums of a successfully completed streaming write.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriteReport {
	/// Logical disk length in bytes.
	pub logical_bytes: u64,
	/// Final container length in bytes.
	pub physical_bytes: u64,
	/// Uncompressed run count.
	pub raw_runs: usize,
	/// Compressed run count for the selected codec.
	pub compressed_runs: usize,
	/// Explicit zero run count after merging adjacent ranges.
	pub zero_runs: usize,
	/// CRC32 of the data fork.
	pub physical_crc: u32,
	/// CRC32 of the complete logical table.
	pub table_crc: u32,
	/// CRC32 of the big-endian table CRC list.
	pub master_crc: u32,
	/// Upper bound charged against the payload budget, excluding output metadata and thread stacks.
	pub accounted_payload_bytes: usize,
}

/// Wrap an exactly sized, sector-aligned logical disk in a single-table UDIF image.
/// Only the input needs a known length. Both stream arguments may be nonseekable.
///
/// # Errors
/// Rejects missing codec features, invalid options, short or overlong sources,
/// resource limits, cancellation and failed I/O. A failed sink may contain an
/// incomplete image prefix. Callers targeting a path should publish through staging.
pub fn write_image(
	mut source: impl Read,
	logical_bytes: u64,
	mut sink: impl Write,
	options: &WriteOptions,
) -> Result<WriteReport, Error> {
	options.cancellation.check()?;
	let max_records = options.max_data_runs(logical_bytes)? + TERMINATOR_RECORD_COUNT;
	let available_workers = match options.execution {
		#[cfg(feature = "parallel")]
		Execution::Parallel {
			workers: Execution::AUTO_WORKERS,
		} => std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
		_ => NonZeroUsize::MIN,
	};
	let admission = options.admit(available_workers)?;
	let scheduler = Scheduler::new(admission)?;
	let mut logical_crc = Hasher::new();
	let mut data = DataFork::new(&mut sink, max_records, options.max_physical_bytes);
	let mut unread = logical_bytes;
	while unread != 0 {
		let mut batch = Vec::with_capacity(scheduler.width());
		for _ in 0..scheduler.width() {
			if unread == 0 {
				break;
			}
			options.cancellation.check()?;
			let size = unread.min(options.chunk_bytes as u64) as usize;
			let mut bytes = vec![0; size];
			source
				.read_exact(&mut bytes)
				.map_err(|err| match err.kind() {
					io::ErrorKind::UnexpectedEof => {
						Error::SourceLength("ended before declared size")
					}
					_ => err.into(),
				})?;
			unread -= size as u64;
			logical_crc.update(&bytes);
			batch.push(RawChunk(bytes));
		}
		for result in scheduler.encode(batch, options) {
			options.cancellation.check()?;
			data.write(result?)?;
		}
	}
	match source.read_exact(&mut [0]) {
		Ok(()) => return Err(Error::SourceLength("has bytes beyond declared size")),
		Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {}
		Err(err) => return Err(err.into()),
	}
	options.cancellation.check()?;
	data.finish(
		logical_crc.finalize(),
		options,
		admission.accounted_payload_bytes(),
	)
}

struct RawChunk(Vec<u8>);

struct StoredChunk {
	sectors: u64,
	kind: Kind,
	bytes: Vec<u8>,
}

struct DataFork<W> {
	sink: W,
	records: Vec<Record>,
	hash: Hasher,
	sectors: u64,
	stored_bytes: u64,
	max_physical_bytes: u64,
}

impl<W: Write> DataFork<W> {
	fn new(sink: W, max_records: usize, max_physical_bytes: u64) -> Self {
		Self {
			sink,
			records: Vec::with_capacity(max_records),
			hash: Hasher::new(),
			sectors: 0,
			stored_bytes: 0,
			max_physical_bytes,
		}
	}

	fn write(&mut self, chunk: StoredChunk) -> Result<(), Error> {
		let len = chunk.bytes.len() as u64;
		let end = self
			.stored_bytes
			.checked_add(len)
			.filter(|&end| end <= self.max_physical_bytes - trailer::BYTES as u64)
			.ok_or(Error::ResourceLimit("physical data fork size"))?;
		self.sink.write_all(&chunk.bytes)?;
		self.hash.update(&chunk.bytes);
		if let Some(last) = self
			.records
			.last_mut()
			.filter(|record| record.kind == Kind::Zero && chunk.kind == Kind::Zero)
		{
			last.sector_count += chunk.sectors;
		} else {
			self.records.push(Record {
				kind: chunk.kind,
				comment: blkx::NO_RUN_COMMENT,
				first_sector: self.sectors,
				sector_count: chunk.sectors,
				offset: self.stored_bytes,
				stored_len: len,
			});
		}
		self.sectors += chunk.sectors;
		self.stored_bytes = end;
		Ok(())
	}

	fn finish(
		mut self,
		table_crc: u32,
		options: &WriteOptions,
		accounted_payload_bytes: usize,
	) -> Result<WriteReport, Error> {
		self.records.push(Record {
			kind: Kind::End,
			comment: blkx::NO_RUN_COMMENT,
			first_sector: self.sectors,
			sector_count: 0,
			offset: self.stored_bytes,
			stored_len: 0,
		});
		let raw_runs = self.count(Kind::Raw);
		let compressed_runs = self
			.records
			.iter()
			.filter(|record| record.kind.is_compressed())
			.count();
		let zero_runs = self.count(Kind::Zero);
		let buffer_sectors = match options.profile {
			Profile::Udro => RAW_ONLY_BUFFER_SECTORS,
			Profile::Udzo => {
				(options.chunk_bytes as u32 / SECTOR_BYTES as u32) + ZLIB_BUFFER_HEADROOM_SECTORS
			}
			// Native ULFO and ULMO declare enough sectors for one compressed run and its
			// decoded extent together, rounded up before taking the maximum.
			Profile::Ulfo => self.combined_buffer_sectors(Kind::Lzfse)?,
			Profile::Ulmo => self.combined_buffer_sectors(Kind::Lzma2)?,
		};
		let physical_crc = self.hash.finalize();
		let master_crc = checksum::master_crc([table_crc]);
		let xml = resource::encode(blkx::encode(
			self.sectors,
			buffer_sectors,
			table_crc,
			&self.records,
		)?)?;
		let physical_bytes = self
			.stored_bytes
			.checked_add(xml.len() as u64 + trailer::BYTES as u64)
			.filter(|bytes| *bytes <= self.max_physical_bytes)
			.ok_or(Error::ResourceLimit("physical image size"))?;
		let trailer = trailer::encode(
			self.sectors,
			self.stored_bytes,
			xml.len() as u64,
			physical_crc,
			master_crc,
		);
		options.cancellation.check()?;
		self.sink.write_all(&xml)?;
		self.sink.write_all(&trailer)?;
		self.sink.flush()?;
		options.cancellation.check()?;
		Ok(WriteReport {
			logical_bytes: self.sectors * SECTOR_BYTES,
			physical_bytes,
			raw_runs,
			compressed_runs,
			zero_runs,
			physical_crc,
			table_crc,
			master_crc,
			accounted_payload_bytes,
		})
	}

	fn combined_buffer_sectors(&self, kind: Kind) -> Result<u32, Error> {
		let sectors = self
			.records
			.iter()
			.filter(|record| record.kind == kind)
			.map(|record| record.sector_count + record.stored_len.div_ceil(SECTOR_BYTES))
			.max()
			.unwrap_or(RAW_ONLY_BUFFER_SECTORS.into());
		u32::try_from(sectors).map_err(|_| Error::ResourceLimit("codec buffer sector count"))
	}

	fn count(&self, kind: Kind) -> usize {
		self.records
			.iter()
			.filter(|record| record.kind == kind)
			.count()
	}
}
