use std::num::NonZeroUsize;

use crate::Error;

use super::{Execution, RawChunk, StoredChunk, WriteAdmission, WriteOptions};

const CODEC_WORKSPACE_BYTES: usize = 1 << 20;
const AUTO_MAX_WORKERS: usize = 8;
const MAX_WORKERS: usize = 32;

pub(super) fn admit(
	options: &WriteOptions,
	available_workers: NonZeroUsize,
) -> Result<WriteAdmission, Error> {
	if !cfg!(feature = "parallel") && matches!(options.execution, Execution::Parallel { .. }) {
		return Err(Error::FeatureNotCompiled("parallel"));
	}
	// Charge encoder scratch and both raw and encoded buffers before admitting any input.
	let per_chunk = match options.profile {
		super::Profile::Udro => options.chunk_bytes,
		super::Profile::Udzo => 2 * options.chunk_bytes + CODEC_WORKSPACE_BYTES,
	};
	let capacity = options.max_buffer_bytes / per_chunk;
	let workers = match options.execution {
		Execution::Serial => NonZeroUsize::MIN,
		Execution::Parallel {
			workers: Execution::AUTO_WORKERS,
		} => NonZeroUsize::new(available_workers.get().min(AUTO_MAX_WORKERS).min(capacity))
			.unwrap_or(NonZeroUsize::MIN),
		Execution::Parallel { workers } => {
			NonZeroUsize::new(workers).expect("automatic scheduling already handled zero workers")
		}
	};
	let width = workers.get();
	if width > MAX_WORKERS || width > capacity {
		return Err(Error::ResourceLimit(
			"worker count or payload buffer budget",
		));
	}
	Ok(WriteAdmission {
		workers,
		accounted_payload_bytes: width * per_chunk,
	})
}

pub(super) struct Scheduler {
	width: NonZeroUsize,
	#[cfg(feature = "parallel")]
	pool: Option<rayon::ThreadPool>,
}

impl Scheduler {
	pub(super) fn new(admission: WriteAdmission) -> Result<Self, Error> {
		let width = admission.workers;
		#[cfg(feature = "parallel")]
		let pool = if width > NonZeroUsize::MIN {
			Some(
				rayon::ThreadPoolBuilder::new()
					.num_threads(width.get())
					.build()
					.map_err(Error::WorkerPool)?,
			)
		} else {
			None
		};
		Ok(Self {
			width,
			#[cfg(feature = "parallel")]
			pool,
		})
	}

	pub(super) fn width(&self) -> usize {
		self.width.get()
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
