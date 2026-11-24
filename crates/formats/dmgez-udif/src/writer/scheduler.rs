use crate::Error;

use super::{Execution, RawChunk, StoredChunk, WriteOptions};

const CODEC_WORKSPACE_BYTES: usize = 1 << 20;
const SERIAL_WIDTH: usize = 1;
#[cfg(feature = "parallel")]
const AUTO_MAX_WORKERS: usize = 8;
const MAX_WORKERS: usize = 32;

pub(super) struct Scheduler {
	width: usize,
	accounted_bytes: usize,
	#[cfg(feature = "parallel")]
	pool: Option<rayon::ThreadPool>,
}

impl Scheduler {
	pub(super) fn new(options: &WriteOptions) -> Result<Self, Error> {
		// Include encoder scratch and both raw and encoded buffers before reading any input.
		let per_chunk = match options.profile {
			super::Profile::Udro => options.chunk_bytes,
			super::Profile::Udzo => 2 * options.chunk_bytes + CODEC_WORKSPACE_BYTES,
		};
		let capacity = options.max_buffer_bytes / per_chunk;
		let width = match options.execution {
			Execution::Serial => SERIAL_WIDTH,
			#[cfg(feature = "parallel")]
			Execution::Parallel {
				workers: Execution::AUTO_WORKERS,
			} => std::thread::available_parallelism()
				.map_or(SERIAL_WIDTH, std::num::NonZero::get)
				.min(AUTO_MAX_WORKERS)
				.min(capacity)
				.max(SERIAL_WIDTH),
			#[cfg(not(feature = "parallel"))]
			Execution::Parallel { .. } => return Err(Error::FeatureNotCompiled("parallel")),
			#[cfg(feature = "parallel")]
			Execution::Parallel { workers } => workers,
		};
		if width > MAX_WORKERS || width > capacity {
			return Err(Error::ResourceLimit(
				"worker count or payload buffer budget",
			));
		}
		#[cfg(feature = "parallel")]
		let pool = if width > SERIAL_WIDTH {
			Some(
				rayon::ThreadPoolBuilder::new()
					.num_threads(width)
					.build()
					.map_err(Error::WorkerPool)?,
			)
		} else {
			None
		};
		Ok(Self {
			width,
			accounted_bytes: width * per_chunk,
			#[cfg(feature = "parallel")]
			pool,
		})
	}

	pub(super) fn width(&self) -> usize {
		self.width
	}

	pub(super) fn accounted_bytes(&self) -> usize {
		self.accounted_bytes
	}

	pub(super) fn encode(
		&self,
		chunks: Vec<RawChunk>,
		options: &WriteOptions,
	) -> Vec<Result<StoredChunk, Error>> {
		#[cfg(feature = "parallel")]
		if let Some(pool) = &self.pool {
			use rayon::prelude::*;
			return pool.install(|| {
				chunks
					.into_par_iter()
					.map(|chunk| chunk.encode(options))
					.collect()
			});
		}
		chunks
			.into_iter()
			.map(|chunk| chunk.encode(options))
			.collect()
	}
}
