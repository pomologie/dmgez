use std::io::{self, Read, Seek, SeekFrom, Write};

use dmgez_core::output::StagedReader;
use dmgez_udif::writer::{Cancellation, WriteOptions};
use thiserror::Error;

const MAX_RAW_BUFFER_BYTES: usize = 64 << 10;

/// A failure emitting or checking a trailerless raw-backed image.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RawWriteError {
	#[error("raw source ended before its declared size")]
	ShortSource,
	#[error("raw source has bytes beyond its declared size")]
	LongSource,
	#[error("raw image write cancelled")]
	Cancelled,
	#[error("could not allocate the admitted raw write buffer")]
	Buffer,
	#[error("staged raw image length {actual} differs from the admitted {expected} bytes")]
	StageLength { expected: u64, actual: u64 },
	#[error("raw image I/O")]
	Io(#[from] io::Error),
}

pub(super) fn write(
	mut source: impl Read,
	logical_bytes: u64,
	mut sink: impl Write,
	options: &WriteOptions,
) -> Result<(), RawWriteError> {
	let cancellation = &options.cancellation;
	check_cancelled(cancellation)?;
	let capacity = options.max_buffer_bytes.min(MAX_RAW_BUFFER_BYTES);
	let mut buffer = Vec::new();
	buffer
		.try_reserve_exact(capacity)
		.map_err(|_| RawWriteError::Buffer)?;
	buffer.resize(capacity, 0);
	let mut remaining = logical_bytes;
	while remaining != 0 {
		check_cancelled(cancellation)?;
		let size = remaining.min(capacity as u64) as usize;
		let chunk = &mut buffer[..size];
		source.read_exact(chunk).map_err(|error| {
			if error.kind() == io::ErrorKind::UnexpectedEof {
				RawWriteError::ShortSource
			} else {
				error.into()
			}
		})?;
		check_cancelled(cancellation)?;
		sink.write_all(chunk)?;
		remaining -= size as u64;
	}
	check_cancelled(cancellation)?;
	match source.read_exact(&mut [0]) {
		Ok(()) => Err(RawWriteError::LongSource),
		Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => check_cancelled(cancellation),
		Err(error) => Err(error.into()),
	}
}

pub(super) fn validate_stage(
	mut stage: StagedReader<'_>,
	expected: u64,
) -> Result<(), RawWriteError> {
	let actual = stage.seek(SeekFrom::End(0))?;
	if actual != expected {
		return Err(RawWriteError::StageLength { expected, actual });
	}
	Ok(())
}

fn check_cancelled(cancellation: &Cancellation) -> Result<(), RawWriteError> {
	if cancellation.is_cancelled() {
		return Err(RawWriteError::Cancelled);
	}
	Ok(())
}
