use std::{error::Error as StdError, fmt};

use dmgez_vfs::volume::{NodeHandle, NodeKind, RawName, VolumeBackend, VolumeView};

use super::{Action, Error, next_page};

const SEARCH_SCAN_LIMIT: u64 = 4_096;
const SEARCH_MATCH_LIMIT: usize = 64;

/// A direct child's original stored name and observed kind.
#[derive(Debug)]
pub struct SearchEntry {
	pub name: RawName,
	pub kind: NodeKind,
}

/// What stopped the bounded directory scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchExtent {
	/// The scan reached the end of the directory within both limits.
	Complete,
	/// The scan used its entry allowance before reaching the directory end.
	ScanLimit,
	/// The scan found another matching entry after filling its result allowance.
	MatchLimit,
}

/// Matches in native directory order, retaining the exact names needed for lookup.
#[derive(Debug)]
pub struct SearchReport {
	pub entries: Vec<SearchEntry>,
	pub scanned_entries: u64,
	pub extent: SearchExtent,
}

/// Directory admission or name comparison failed during the search.
#[derive(Debug)]
#[non_exhaustive]
pub enum SearchError<E> {
	EmptyQuery,
	Directory(Error<E>),
	Match(E),
}

impl<E> fmt::Display for SearchError<E> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::EmptyQuery => formatter.write_str("enter a searchable name fragment"),
			Self::Directory(error) => fmt::Display::fmt(error, formatter),
			Self::Match(_) => formatter.write_str("could not compare a stored name"),
		}
	}
}

impl<E: StdError + 'static> StdError for SearchError<E> {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		match self {
			Self::EmptyQuery => None,
			Self::Directory(error) => Some(error),
			Self::Match(error) => Some(error),
		}
	}
}

impl<E> From<Error<E>> for SearchError<E> {
	fn from(error: Error<E>) -> Self {
		Self::Directory(error)
	}
}

/// Searches at most 4,096 direct catalog entries and returns up to 64 matches.
///
/// The predicate decides name comparison for the selected filesystem. The
/// directory handle must come from this view. Child handles are never opened.
///
/// # Errors
/// Propagates directory and comparison failures, including backend I/O limits.
pub fn search_children<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	parent: &NodeHandle<B::Id>,
	mut matches: impl FnMut(&RawName) -> Result<bool, B::Error>,
) -> Result<SearchReport, SearchError<B::Error>> {
	let mut directory = view
		.read_dir(parent)
		.map_err(|error| Error::view(Action::Directory, error))?;
	let mut entries = Vec::new();
	entries
		.try_reserve_exact(SEARCH_MATCH_LIMIT)
		.map_err(Error::from)?;
	let mut scanned_entries = 0;
	let extent = 'scan: loop {
		let page = next_page(&mut directory, SEARCH_SCAN_LIMIT - scanned_entries)?;
		for entry in page.entries {
			scanned_entries += 1;
			if matches(&entry.name).map_err(SearchError::Match)? {
				if entries.len() == SEARCH_MATCH_LIMIT {
					break 'scan SearchExtent::MatchLimit;
				}
				entries.push(SearchEntry {
					name: entry.name,
					kind: entry.kind,
				});
			}
		}
		if page.complete {
			break SearchExtent::Complete;
		}
		if scanned_entries == SEARCH_SCAN_LIMIT {
			break SearchExtent::ScanLimit;
		}
	};
	Ok(SearchReport {
		entries,
		scanned_entries,
		extent,
	})
}
