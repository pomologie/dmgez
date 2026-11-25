use std::{
	io::{self, Read, Write},
	num::NonZeroUsize,
};

use crc32fast::Hasher;

use crate::Error;

#[cfg(feature = "parallel")]
use super::Execution;
use super::{
	DataFork, OutputMetadata, RawChunk, TERMINATOR_RECORD_COUNT, WriteOptions, WriteReport,
	scheduler::Scheduler,
};

pub(super) fn write_stream(
	mut source: impl Read,
	logical_bytes: u64,
	mut sink: impl Write,
	options: &WriteOptions,
	metadata: OutputMetadata,
	#[cfg(feature = "write-cache")] cache: Option<&super::cache::ActiveCache<'_>>,
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
	#[cfg(feature = "write-cache")]
	let admission = if cache.is_some() {
		options.admit_with_cache(available_workers)?
	} else {
		options.admit(available_workers)?
	};
	#[cfg(not(feature = "write-cache"))]
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
		for result in scheduler.encode(
			batch,
			options,
			#[cfg(feature = "write-cache")]
			cache,
		) {
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
		metadata,
	)
}
