//! Streaming UDRO and UDZO construction from finite logical disk bytes.

mod scheduler;

use std::{
	io::{self, Read, Write},
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
const TERMINATOR_RECORD_COUNT: u64 = 1;
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
	/// Zlib run count.
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
	let max_runs = admit(logical_bytes, options)?;
	let scheduler = Scheduler::new(options)?;
	let mut logical_crc = Hasher::new();
	let mut data = DataFork::new(&mut sink, max_runs, options.max_physical_bytes);
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
	data.finish(logical_crc.finalize(), options, scheduler.accounted_bytes())
}

fn admit(logical_bytes: u64, options: &WriteOptions) -> Result<usize, Error> {
	if logical_bytes == 0 || !logical_bytes.is_multiple_of(SECTOR_BYTES) {
		return Err(Error::UnalignedSource);
	}
	if logical_bytes > options.max_logical_bytes {
		return Err(Error::ResourceLimit("logical disk size"));
	}
	if !(SECTOR_BYTES as usize..=MAX_CHUNK_BYTES).contains(&options.chunk_bytes)
		|| !options.chunk_bytes.is_multiple_of(SECTOR_BYTES as usize)
	{
		return Err(Error::ResourceLimit(
			"chunk size must be sector-aligned and at most 1 MiB",
		));
	}
	if options.zlib_level > MAX_ZLIB_LEVEL {
		return Err(Error::ResourceLimit("zlib level must be between 0 and 9"));
	}
	if options.profile == Profile::Udzo && !cfg!(feature = "zlib") {
		return Err(Error::FeatureNotCompiled("zlib"));
	}
	let records = logical_bytes.div_ceil(options.chunk_bytes as u64) + TERMINATOR_RECORD_COUNT;
	if records > MAX_RECORDS as u64 {
		return Err(Error::ResourceLimit("maximum block-map records"));
	}
	if options.max_physical_bytes <= trailer::BYTES as u64 {
		return Err(Error::ResourceLimit("physical image size"));
	}
	Ok(records as usize)
}

struct RawChunk(Vec<u8>);

struct StoredChunk {
	sectors: u64,
	kind: Kind,
	bytes: Vec<u8>,
}

impl RawChunk {
	fn encode(self, options: &WriteOptions) -> Result<StoredChunk, Error> {
		options.cancellation.check()?;
		let sectors = self.0.len() as u64 / SECTOR_BYTES;
		if options.profile == Profile::Udzo {
			if self.0.iter().all(|&byte| byte == 0) {
				return Ok(StoredChunk {
					sectors,
					kind: Kind::Zero,
					bytes: Vec::new(),
				});
			}
			#[cfg(feature = "zlib")]
			if let Some(bytes) = crate::codec::deflate(&self.0, options.zlib_level)? {
				return Ok(StoredChunk {
					sectors,
					kind: Kind::Zlib,
					bytes,
				});
			}
		}
		Ok(StoredChunk {
			sectors,
			kind: Kind::Raw,
			bytes: self.0,
		})
	}
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
		let compressed_runs = self.count(Kind::Zlib);
		let zero_runs = self.count(Kind::Zero);
		let physical_crc = self.hash.finalize();
		let master_crc = checksum::master_crc([table_crc]);
		let buffer_sectors = match options.profile {
			Profile::Udro => RAW_ONLY_BUFFER_SECTORS,
			Profile::Udzo => {
				(options.chunk_bytes as u32 / SECTOR_BYTES as u32) + ZLIB_BUFFER_HEADROOM_SECTORS
			}
		};
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

	fn count(&self, kind: Kind) -> usize {
		self.records
			.iter()
			.filter(|record| record.kind == kind)
			.count()
	}
}
