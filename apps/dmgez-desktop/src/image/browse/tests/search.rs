use dmgez_inspector::volume_queries::{SearchError, SearchExtent};
use dmgez_udif::Cancellation;
use dmgez_vfs::{
	VolumeId,
	volume::{NodeKind, RawName},
};

use super::{
	CATALOG, HFSX, NATIVE_CATALOG_ENTRIES, activate, fixture, materialize_sparse, page, wrap,
};
use crate::{
	image::{
		ImageTab,
		browse::{Error, Location, Observation, Request, search::Report},
	},
	inspector::Action,
};

fn search(tab: &ImageTab, location: Location, query: &str) -> Report {
	let result = tab
		.snapshot
		.browse(
			Request::Search {
				location,
				query: query.into(),
			},
			&Cancellation::default(),
		)
		.unwrap();
	let Observation::Search(report) = result else {
		panic!("expected search results");
	};
	report
}

#[test]
fn finds_an_exact_stored_name_beyond_the_initial_page_and_inspects_it() {
	let mut tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	let first = page(&tab, root.clone(), 0);
	let expected = RawName::Utf16("item-095".encode_utf16().collect());
	assert!(first.entries.iter().all(|entry| entry.name != expected));
	tab.browser.showing = true;
	assert!(tab.browser.show_page(first, None));
	tab.browser.search.query = "ITEM-095".into();

	let Action::Browse(request @ Request::Search { .. }) = activate(&mut tab, "Search") else {
		panic!("expected to search the currently open folder");
	};
	let Observation::Search(report) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected search results");
	};
	assert_eq!(report.extent, SearchExtent::Complete);
	assert_eq!(report.scanned_entries, NATIVE_CATALOG_ENTRIES as u64);
	assert_eq!(report.entries.len(), 1);
	assert_eq!(report.entries[0].name, expected);
	tab.browser.search.apply(&root, report);

	let Action::Browse(request @ Request::Details { .. }) =
		activate(&mut tab, "Details for item-095")
	else {
		panic!("expected details for the exact search result");
	};
	let Observation::Details(details) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected details of the exact stored child");
	};
	assert_eq!(details.name, expected);
	assert_eq!(details.metadata.kind, NodeKind::RegularFile);
}

#[test]
fn search_preserves_decomposed_names_and_uses_the_volume_case_profile() {
	let tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	assert!(search(&tab, root.clone(), "ÉSUMÉ").entries.is_empty());
	let folder = root
		.child(&RawName::Utf16("Nested".encode_utf16().collect()))
		.unwrap();
	let report = search(&tab, folder, "ÉSUMÉ");
	assert_eq!(report.extent, SearchExtent::Complete);
	assert_eq!(report.entries.len(), 1);
	assert_eq!(
		report.entries[0].name,
		RawName::Utf16("Re\u{301}sume\u{301}.txt".encode_utf16().collect()),
	);

	let hfsx = wrap(&materialize_sparse(&fixture(HFSX)));
	let report = search(&hfsx, root, "Case");
	assert_eq!(report.extent, SearchExtent::Complete);
	assert_eq!(report.entries.len(), 1);
	assert_eq!(
		report.entries[0].name,
		RawName::Utf16("Cases".encode_utf16().collect()),
	);
}

#[test]
fn caps_matches_refuses_ignorable_queries_and_rejects_cancellation() {
	let tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	let report = search(&tab, root.clone(), "item-");
	assert_eq!(report.extent, SearchExtent::MatchLimit);
	assert_eq!(report.entries.len(), 64);
	assert!(report.scanned_entries < NATIVE_CATALOG_ENTRIES as u64);

	let cancellation = Cancellation::default();
	let request = Request::Search {
		location: root.clone(),
		query: "\u{200c}".into(),
	};
	assert!(matches!(
		tab.snapshot.browse(request, &cancellation),
		Err(Error::Search(SearchError::EmptyQuery))
	));
	cancellation.cancel();
	let request = Request::Search {
		location: root,
		query: "item-095".into(),
	};
	assert!(matches!(
		tab.snapshot.browse(request, &cancellation),
		Err(Error::Image(dmgez_udif::Error::Cancelled))
	));
}
