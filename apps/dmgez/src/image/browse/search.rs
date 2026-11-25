//! Held-image routing and stale result admission for folder search.

use dmgez_hfsplus_read::browse::{self, NameFragment};
use dmgez_inspector::volume_queries::{SearchError, SearchExtent, search_children};
use dmgez_vfs::volume::{RawName, VolumeBackend, VolumeView};

use super::{Entry, Error, Location, folder};

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

pub(super) fn folder_search(
	view: &mut VolumeView<impl VolumeBackend<Error = browse::Error>>,
	location: Location,
	query: String,
) -> Result<super::Observation, Error> {
	let fragment =
		NameFragment::new(view.native_name_profile(), &query)?.ok_or(SearchError::EmptyQuery)?;
	let parent = folder(view, &location)?;
	let result = search_children(view, &parent, |name| match name {
		RawName::Utf16(units) => fragment.matches(units),
		_ => Err(browse::Error::LookupEncoding),
	})?;
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
