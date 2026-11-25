//! Held-image routing and stale result admission for folder search.

#[cfg(feature = "hfsplus-read")]
use dmgez_hfsplus::browse::{self, NameFragment};
use dmgez_inspector::volume_queries::{SearchError, SearchExtent, search_children};
use dmgez_vfs::volume::{RawName, ViewError, VolumeBackend, VolumeView};

use super::{Entry, Error, Location, query::folder};

#[derive(Default)]
pub(crate) struct State {
	pub query: String,
	pub report: Option<Report>,
}

impl State {
	pub(crate) fn apply(&mut self, location: &Location, report: Report) {
		if *location == report.location && self.query == report.query {
			self.report = Some(report);
		}
	}

	pub(crate) fn clear(&mut self) {
		self.query.clear();
		self.report = None;
	}
}

#[derive(Debug)]
pub(crate) struct Report {
	pub location: Location,
	pub query: String,
	pub entries: Vec<Entry>,
	pub scanned_entries: u64,
	pub extent: SearchExtent,
}

#[cfg(feature = "hfsplus-read")]
pub(super) fn hfs(
	view: &mut VolumeView<impl VolumeBackend<Error = browse::Error>>,
	location: Location,
	query: String,
) -> Result<super::Observation, Error> {
	let fragment = NameFragment::new(view.native_name_profile(), &query)?
		.ok_or(SearchError::<browse::Error>::EmptyQuery)?;
	folder_search(view, location, query, |name, _| match name {
		RawName::Utf16(units) => fragment.matches(units),
		_ => Err(browse::Error::LookupEncoding),
	})
}

pub(super) fn folder_search<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	location: Location,
	query: String,
	mut matches: impl FnMut(&RawName, &str) -> Result<bool, B::Error>,
) -> Result<super::Observation, Error>
where
	Error: From<ViewError<B::Error>> + From<SearchError<B::Error>>,
{
	if query.is_empty() {
		return Err(SearchError::<B::Error>::EmptyQuery.into());
	}
	let parent = folder(view, &location)?;
	let result = search_children(view, &parent, |name| matches(name, &query))?;
	Ok(super::Observation::Search(Report {
		location,
		query,
		entries: result
			.entries
			.into_iter()
			.map(|entry| Entry {
				name: entry.name,
				kind: entry.kind,
			})
			.collect(),
		scanned_entries: result.scanned_entries,
		extent: result.extent,
	}))
}
