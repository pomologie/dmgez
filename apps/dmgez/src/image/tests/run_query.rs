use dmgez_inspector::RunEncoding;
use dmgez_runtime::image::{RunFilter, RunOrder, RunQuery};

use super::{Action, ImageTab, activate_inspector};
use crate::image::{RUN_PAGE_SIZE, exports::Kind};
use dmgez_udif::{
	Cancellation, SECTOR_BYTES,
	writer::{Profile, WriteOptions, write_image},
};

const RUNS_PER_CODEC: usize = RUN_PAGE_SIZE + 2;
const TOTAL_RUNS: usize = RUNS_PER_CODEC * 2;
const PAYLOAD_BYTE: u8 = b'R';

fn mixed_runs() -> ImageTab {
	let mut stored = tempfile::tempfile().unwrap();
	let mut logical = vec![0; TOTAL_RUNS * SECTOR_BYTES as usize];
	for pair in logical.chunks_exact_mut((2 * SECTOR_BYTES) as usize) {
		pair[SECTOR_BYTES as usize..].fill(PAYLOAD_BYTE);
	}
	write_image(
		logical.as_slice(),
		logical.len() as u64,
		&mut stored,
		&WriteOptions {
			profile: Profile::Udzo,
			chunk_bytes: SECTOR_BYTES as usize,
			..WriteOptions::default()
		},
	)
	.unwrap();
	ImageTab::open(stored).unwrap()
}

#[test]
fn sorts_and_pages_the_whole_index_while_retaining_original_ordinals() {
	let tab = mixed_runs();
	let cancel = Cancellation::default();
	assert_eq!(tab.report.container.runs.total, TOTAL_RUNS);
	for order in [RunOrder::LargestStored, RunOrder::StoredOffset] {
		let sorted = RunQuery {
			filter: RunFilter::All,
			order,
		};
		let first = tab.snapshot.query_runs(sorted, 0, &cancel).unwrap();
		assert_eq!(first.total_matches, TOTAL_RUNS);
		assert_eq!(first.rows.len(), RUN_PAGE_SIZE);
		assert!(
			first
				.rows
				.iter()
				.zip((1..).step_by(2))
				.all(|(row, ordinal)| {
					row.ordinal == ordinal && row.run.encoding == RunEncoding::Zlib
				})
		);
		let second = tab
			.snapshot
			.query_runs(sorted, first.next_offset.unwrap(), &cancel)
			.unwrap();
		assert_eq!(second.rows[0].ordinal, RUN_PAGE_SIZE * 2 + 1);
		assert_eq!(second.rows[1].ordinal, RUN_PAGE_SIZE * 2 + 3);
		assert_eq!(second.rows[2].ordinal, 0);
		assert_eq!(second.rows[2].run.encoding, RunEncoding::Zero);
	}
}

#[test]
fn accessible_query_controls_keep_filtered_ordinals_exportable_and_reset_to_logical_order() {
	let mut tab = mixed_runs();
	let cancel = Cancellation::default();
	let query = RunQuery {
		filter: RunFilter::Zlib,
		order: RunOrder::LargestStored,
	};
	tab.run_navigation.query = query;
	assert!(matches!(
		activate_inspector(&mut tab, "Apply run query"),
		Some(Action::QueryRuns { query: applied, offset: 0 }) if applied == query
	));
	tab.run_navigation.query_page = Some(tab.snapshot.query_runs(query, 0, &cancel).unwrap());
	assert!(matches!(
		activate_inspector(&mut tab, "Next page"),
		Some(Action::QueryRuns { query: applied, offset: RUN_PAGE_SIZE }) if applied == query
	));
	let page = tab
		.snapshot
		.query_runs(query, RUN_PAGE_SIZE, &cancel)
		.unwrap();
	assert_eq!(page.total_matches, RUNS_PER_CODEC);
	assert_eq!(page.rows.len(), RUNS_PER_CODEC - RUN_PAGE_SIZE);
	assert_eq!(page.next_offset, None);
	let ordinal = page.rows[0].ordinal;
	tab.run_navigation.query_page = Some(page);
	assert!(activate_inspector(&mut tab, &format!("Select run {}", ordinal + 1)).is_none());
	assert_eq!(tab.run_navigation.selected, Some(ordinal));
	let output = tempfile::tempdir().unwrap();
	let destination = output.path().join("run.raw");
	let export = tab.export(Kind::Run(ordinal)).unwrap();
	export.execute(&destination, &cancel).unwrap();
	assert_eq!(
		std::fs::read(destination).unwrap(),
		vec![PAYLOAD_BYTE; SECTOR_BYTES as usize]
	);
	assert!(tab.export(Kind::Run(0)).is_none());
	assert!(matches!(
		activate_inspector(&mut tab, "Clear run query"),
		Some(Action::Page(0))
	));
}
