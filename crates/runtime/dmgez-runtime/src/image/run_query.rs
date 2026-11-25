use std::cmp::Reverse;

use dmgez_inspector::{Run, RunWindow};
use dmgez_udif::{
	Cancellation, Error,
	reader::{ReadAt, RunDescriptor, RunEncoding},
};

use super::ImageSession;

// Bound this operation independently of any larger reader limits selected by
// library callers. This covers the entire default UDIF record limit.
const MAX_QUERY_RUNS: usize = 131_072;
const QUERY_INDEX_BUDGET: &str = "run query index";
const QUERY_PAGE_BUDGET: &str = "run query page";
const QUERY_ORDINAL: &str = "run query ordinal";

/// Limits results by the admitted run encoding or the current decoder coverage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunFilter {
	#[default]
	All,
	Raw,
	Zero,
	Ignore,
	Zlib,
	Lzfse,
	Lzma2,
	DecoderUnavailable,
}

impl RunFilter {
	fn matches(self, encoding: RunEncoding) -> bool {
		match self {
			Self::All => true,
			Self::Raw => encoding == RunEncoding::Raw,
			Self::Zero => encoding == RunEncoding::Zero,
			Self::Ignore => encoding == RunEncoding::Ignore,
			Self::Zlib => encoding == RunEncoding::Zlib,
			Self::Lzfse => encoding == RunEncoding::Lzfse,
			Self::Lzma2 => encoding == RunEncoding::Lzma2,
			Self::DecoderUnavailable => !encoding.can_decode(),
		}
	}
}

/// Run result ordering. Equal keys retain their original logical ordering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunOrder {
	#[default]
	Logical,
	LargestLogical,
	LargestStored,
	/// Increasing stored byte offset, with runs without stored bytes at the end.
	StoredOffset,
}

/// A container-wide run filter and ordering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunQuery {
	pub filter: RunFilter,
	pub order: RunOrder,
}

/// One row with its original zero-based logical ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunQueryRow {
	pub ordinal: usize,
	pub run: Run,
}

/// Bounded table page in the requested order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunQueryPage {
	pub query: RunQuery,
	/// Offset in matching rows, independent of their logical ordinals.
	pub offset: usize,
	pub next_offset: Option<usize>,
	pub total_matches: usize,
	pub rows: Vec<RunQueryRow>,
}

struct Rank {
	ordinal: usize,
	key: Option<u64>,
}

impl RunOrder {
	fn key(self, run: &RunDescriptor) -> Option<u64> {
		match self {
			Self::Logical => None,
			Self::LargestLogical => Some(run.logical.end - run.logical.start),
			Self::LargestStored => Some(run.physical.end - run.physical.start),
			Self::StoredOffset => (!run.physical.is_empty()).then_some(run.physical.start),
		}
	}

	fn sort(self, rows: &mut [Rank]) {
		match self {
			Self::Logical => {}
			Self::LargestLogical | Self::LargestStored => {
				rows.sort_unstable_by_key(|row| (Reverse(row.key), row.ordinal));
			}
			Self::StoredOffset => {
				rows.sort_unstable_by_key(|row| (row.key.is_none(), row.key, row.ordinal));
			}
		}
	}
}

impl<S: ReadAt> ImageSession<S> {
	/// Filters and orders admitted run metadata without reading or decoding payloads.
	///
	/// The matched ordinals cover the entire admitted index within the query
	/// limit. A page keeps the original logical ordinals used for exact exports.
	///
	/// # Errors
	/// Returns errors for invalid page limits, an exhausted allocation or index
	/// budget, cancellation, or a backing source that fails revalidation.
	pub fn query_runs(
		&self,
		query: RunQuery,
		offset: usize,
		page_size: usize,
		cancellation: &Cancellation,
	) -> Result<RunQueryPage, Error> {
		RunWindow::new(offset, page_size).ok_or(Error::ResourceLimit(QUERY_PAGE_BUDGET))?;
		check_cancelled(cancellation)?;
		self.image.check_source_unchanged()?;
		let runs = self.image.runs();
		if runs.len() > MAX_QUERY_RUNS {
			return Err(Error::ResourceLimit(QUERY_INDEX_BUDGET));
		}
		let mut matching = Vec::new();
		matching
			.try_reserve_exact(runs.len())
			.map_err(|_| Error::ResourceLimit(QUERY_INDEX_BUDGET))?;
		for (ordinal, run) in runs.enumerate() {
			check_cancelled(cancellation)?;
			if query.filter.matches(run.encoding) {
				matching.push(Rank {
					ordinal,
					key: query.order.key(&run),
				});
			}
		}
		query.order.sort(&mut matching);
		check_cancelled(cancellation)?;
		let total_matches = matching.len();
		let mut rows = Vec::new();
		rows.try_reserve_exact(page_size.min(total_matches.saturating_sub(offset)))
			.map_err(|_| Error::ResourceLimit(QUERY_PAGE_BUDGET))?;
		for rank in matching.into_iter().skip(offset).take(page_size) {
			let run = self
				.image
				.run_at(rank.ordinal)
				.ok_or(Error::ResourceLimit(QUERY_ORDINAL))?;
			rows.push(RunQueryRow {
				ordinal: rank.ordinal,
				run: run.into(),
			});
		}
		let next = offset.saturating_add(rows.len());
		self.image.check_source_unchanged()?;
		check_cancelled(cancellation)?;
		Ok(RunQueryPage {
			query,
			offset,
			next_offset: (next < total_matches).then_some(next),
			total_matches,
			rows,
		})
	}
}

fn check_cancelled(cancellation: &Cancellation) -> Result<(), Error> {
	if cancellation.is_cancelled() {
		Err(Error::Cancelled)
	} else {
		Ok(())
	}
}
