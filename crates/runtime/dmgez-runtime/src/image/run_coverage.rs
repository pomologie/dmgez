use std::{num::NonZeroU16, ops::Range};

use dmgez_udif::{Cancellation, Error, reader::ReadAt};
use dmgez_vfs::volume::FileDataRanges;

use super::{ImageSession, RunQueryRow, run_query::check_cancelled};

/// Maximum number of volume-relative file regions in a coverage request.
pub const MAX_FILE_DATA_RANGES: NonZeroU16 = NonZeroU16::new(256).unwrap();
const MAX_COVERING_ROWS: usize = 64;
const COVERAGE_BUDGET: &str = "file run coverage";
const COVERAGE_RANGE: &str = "file data range is empty or outside the admitted volume";
const COVERAGE_INDEX: &str = "file data range is outside the admitted run index";

/// Evidence of the UDIF runs covering one regular file's logical data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunCoverage {
	Mapped(CoveredRuns),
	Unavailable,
	Limit,
}

/// The first covering runs in logical disk order and the full distinct run count.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CoveredRuns {
	pub rows: Vec<RunQueryRow>,
	pub total: usize,
}

impl<S: ReadAt> ImageSession<S> {
	/// Finds covering runs from admitted volume-relative ranges without reading payload bytes.
	/// Overlapping coverage is deduplicated. Filesystem holes and allocation slack
	/// are excluded by the filesystem observation. Only the first 64 rows are retained.
	///
	/// # Errors
	/// Returns an error for invalid bounds, allocation failure, cancellation, or
	/// a source which fails revalidation.
	pub fn cover_file_data(
		&self,
		window: Range<u64>,
		data: &FileDataRanges,
		cancellation: &Cancellation,
	) -> Result<RunCoverage, Error> {
		check_cancelled(cancellation)?;
		self.image.check_source_unchanged()?;
		let coverage = match data {
			FileDataRanges::Mapped(ranges)
				if ranges.len() <= usize::from(MAX_FILE_DATA_RANGES.get()) =>
			{
				RunCoverage::Mapped(self.covered_runs(window, ranges, cancellation)?)
			}
			FileDataRanges::Mapped(_) | FileDataRanges::Limit => RunCoverage::Limit,
			_ => RunCoverage::Unavailable,
		};
		self.image.check_source_unchanged()?;
		check_cancelled(cancellation)?;
		Ok(coverage)
	}

	fn covered_runs(
		&self,
		window: Range<u64>,
		ranges: &[Range<u64>],
		cancellation: &Cancellation,
	) -> Result<CoveredRuns, Error> {
		let length = window
			.end
			.checked_sub(window.start)
			.ok_or(Error::Corrupt(COVERAGE_RANGE))?;
		if window.end > self.image.descriptor().logical_bytes {
			return Err(Error::Corrupt(COVERAGE_RANGE));
		}
		let mut intervals = Vec::new();
		intervals
			.try_reserve_exact(ranges.len())
			.map_err(|_| Error::ResourceLimit(COVERAGE_BUDGET))?;
		for range in ranges {
			check_cancelled(cancellation)?;
			if range.is_empty() || range.end > length {
				return Err(Error::Corrupt(COVERAGE_RANGE));
			}
			let first = self
				.image
				.run_ordinal_at(window.start + range.start)
				.ok_or(Error::Corrupt(COVERAGE_INDEX))?;
			let last = self
				.image
				.run_ordinal_at(window.start + (range.end - 1))
				.ok_or(Error::Corrupt(COVERAGE_INDEX))?;
			intervals.push((first, last + 1));
		}
		intervals.sort_unstable();
		let mut rows = Vec::new();
		let mut total = 0;
		let mut seen_end = 0;
		for (start, end) in intervals {
			check_cancelled(cancellation)?;
			let first = start.max(seen_end);
			let count = end.saturating_sub(first);
			total += count;
			seen_end = seen_end.max(end);
			let take = count.min(MAX_COVERING_ROWS - rows.len());
			rows.try_reserve(take)
				.map_err(|_| Error::ResourceLimit(COVERAGE_BUDGET))?;
			for ordinal in first..first + take {
				let run = self
					.image
					.run_at(ordinal)
					.ok_or(Error::Corrupt(COVERAGE_INDEX))?;
				rows.push(RunQueryRow {
					ordinal,
					run: run.into(),
				});
			}
		}
		Ok(CoveredRuns { rows, total })
	}
}
