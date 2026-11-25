use std::fmt;

use dmgez_vfs::LogicalDisk;
use serde::{Serialize, Serializer};
use thiserror::Error;

const CHUNK_BYTES: u64 = 64 * 1024;

/// Version of the serialized byte comparison.
pub const SCHEMA_VERSION: u32 = 1;
/// Default maximum number of paired bytes compared from each input.
pub const DEFAULT_COMPARE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// The outcome at the end of the verified common prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Equal,
	ByteMismatch,
	LengthMismatch,
}

impl Outcome {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Equal => "equal",
			Self::ByteMismatch => "byte_mismatch",
			Self::LengthMismatch => "length_mismatch",
		}
	}
}

/// Exact result in the address space selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Report {
	pub schema_version: u32,
	pub outcome: Outcome,
	#[serde(serialize_with = "decimal")]
	pub left_bytes: u64,
	#[serde(serialize_with = "decimal")]
	pub right_bytes: u64,
	/// Equal byte count before the first mismatch or either source's EOF.
	#[serde(serialize_with = "decimal")]
	pub common_prefix_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Side {
	Left,
	Right,
}

impl fmt::Display for Side {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Left => "left",
			Self::Right => "right",
		})
	}
}

/// A failed or incomplete comparison has no equality result.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("could not read the {side} source at byte {offset}")]
	Read {
		side: Side,
		offset: u64,
		#[source]
		source: dmgez_vfs::Error,
	},
	#[error("comparison reached its {limit} bytes per input limit without a result")]
	Limit { limit: u64 },
	#[error("comparison cancelled")]
	Cancelled,
}

/// Compares at most `max_bytes_per_input` paired bytes of the supplied sources.
/// Equal results require every byte. A decided mismatch requires only its prefix.
/// Both admitted source identities are rechecked before returning a result.
/// Filesystem interpretation and container verification are separate operations.
///
/// # Errors
/// Returns a read failure, exhausted budget, or cancellation without a report.
pub fn compare(
	left: &mut impl LogicalDisk,
	right: &mut impl LogicalDisk,
	max_bytes_per_input: u64,
	mut cancelled: impl FnMut() -> bool,
) -> Result<Report, Error> {
	let left_bytes = left.len();
	let right_bytes = right.len();
	let paired_bytes = left_bytes.min(right_bytes);
	let scan_bytes = paired_bytes.min(max_bytes_per_input);
	let scratch_bytes = scan_bytes.min(CHUNK_BYTES) as usize;
	let mut left_scratch = vec![0; scratch_bytes];
	let mut right_scratch = vec![0; scratch_bytes];
	let mut offset = 0;
	let mut outcome = if left_bytes == right_bytes {
		Outcome::Equal
	} else {
		Outcome::LengthMismatch
	};
	while offset < scan_bytes {
		if cancelled() {
			return Err(Error::Cancelled);
		}
		let width = (scan_bytes - offset).min(CHUNK_BYTES) as usize;
		let left_chunk = &mut left_scratch[..width];
		let right_chunk = &mut right_scratch[..width];
		read(left, Side::Left, offset, left_chunk)?;
		read(right, Side::Right, offset, right_chunk)?;
		if let Some(first) = left_chunk
			.iter()
			.zip(right_chunk.iter())
			.position(|(l, r)| l != r)
		{
			offset += first as u64;
			outcome = Outcome::ByteMismatch;
			break;
		}
		offset += width as u64;
	}
	if cancelled() {
		return Err(Error::Cancelled);
	}
	if outcome != Outcome::ByteMismatch && scan_bytes < paired_bytes {
		return Err(Error::Limit {
			limit: max_bytes_per_input,
		});
	}
	read(left, Side::Left, left_bytes, &mut [])?;
	read(right, Side::Right, right_bytes, &mut [])?;
	Ok(Report {
		schema_version: SCHEMA_VERSION,
		outcome,
		left_bytes,
		right_bytes,
		common_prefix_bytes: offset,
	})
}

fn read(
	input: &mut impl LogicalDisk,
	side: Side,
	offset: u64,
	output: &mut [u8],
) -> Result<(), Error> {
	input
		.read_exact_at(offset, output)
		.map_err(|source| Error::Read {
			side,
			offset,
			source: source.into(),
		})
}

pub(super) fn decimal<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.collect_str(value)
}
