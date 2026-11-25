use dmgez_runtime::image::RunCoverage;
use dmgez_udif::Cancellation;
use dmgez_vfs::{VolumeId, volume::RawName};

use crate::image::{
	ImageTab,
	browse::{FileRuns, Location, Observation, Request},
};
use crate::inspector::Action;

use super::{activate, fixture, materialize_sparse, page, wrap};

const HARD_LINKS: &str = "native-chained-hardlinks.bin";
const HARD_LINK_DATA: &[u8] = b"pomologie linked file\n";
const COMPRESSED: &str = "native-decmpfs-zlib.bin";

fn covering(tab: &ImageTab, request: Request) -> FileRuns {
	let Observation::FileRuns(report) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected coverage of the selected file data");
	};
	report
}

#[test]
fn opens_covering_runs_for_the_exact_native_hard_link_data_from_file_details() {
	let raw = materialize_sparse(&fixture(HARD_LINKS));
	let data_offset = raw
		.windows(HARD_LINK_DATA.len())
		.position(|bytes| bytes == HARD_LINK_DATA)
		.unwrap() as u64;
	let mut tab = wrap(&raw);
	let expected = tab.snapshot.locate_run(data_offset).unwrap();
	let root = Location::root(VolumeId::WholeDisk);
	tab.browser.showing = true;
	let observed = page(&tab, root.clone(), 0);
	assert!(tab.browser.show_page(observed, None));
	let Action::Browse(request @ Request::Details { .. }) = activate(&mut tab, "Details for one")
	else {
		panic!("expected hard-link details action");
	};
	let Observation::Details(details) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected hard-link details");
	};
	tab.browser.details = Some(details);
	let Action::Browse(request @ Request::FileRuns { .. }) =
		activate(&mut tab, "Show covering runs")
	else {
		panic!("expected an exact file coverage request");
	};
	let file = covering(&tab, request);
	let RunCoverage::Mapped(report) = &file.coverage else {
		panic!("expected hard-link data ranges");
	};
	assert_eq!(report.total, 1);
	assert_eq!(report.rows[0].ordinal, expected.ordinal);
	let alias = covering(
		&tab,
		Request::FileRuns {
			folder: root,
			name: RawName::Utf16("two".encode_utf16().collect()),
		},
	);
	assert_eq!(file.coverage, alias.coverage);
	tab.browser.showing = false;
	tab.run_navigation.file_runs = Some(file);
	assert!(matches!(
		activate(&mut tab, "Show all runs"),
		Action::Page(0)
	));
}

#[test]
fn compressed_logical_data_has_explicitly_unavailable_coverage() {
	let tab = wrap(&materialize_sparse(&fixture(COMPRESSED)));
	let report = covering(
		&tab,
		Request::FileRuns {
			folder: Location::root(VolumeId::WholeDisk),
			name: RawName::Utf16("resource".encode_utf16().collect()),
		},
	);
	assert_eq!(report.coverage, RunCoverage::Unavailable);
}
