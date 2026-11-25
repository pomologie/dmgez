use std::cell::Cell;

use super::*;

const UDZO: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");

#[derive(Default)]
struct Fixture {
	closed_to_reads: Cell<bool>,
	revalidations: Cell<usize>,
	changed: Cell<bool>,
	change_at_check: Cell<Option<usize>>,
}

impl ReadAt for &Fixture {
	fn len(&self) -> u64 {
		UDZO.len() as u64
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.revalidations.set(self.revalidations.get() + 1);
		if self.changed.get() || self.change_at_check.get() == Some(self.revalidations.get()) {
			Err(Error::SourceChanged)
		} else {
			Ok(())
		}
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		assert!(
			!self.closed_to_reads.get(),
			"run lookup read the image again"
		);
		UDZO.read_exact_at(offset, output)
	}
}

#[test]
fn querying_runs_only_reads_retained_metadata_and_revalidates_before_delivery() {
	use dmgez_udif::Cancellation;

	let fixture = Fixture::default();
	let session =
		ImageSession::open(&fixture, Limits::default(), MetadataBudget::ReaderLimits).unwrap();
	fixture.closed_to_reads.set(true);
	let expected = session.run_page(RunWindow::new(0, 1).unwrap()).unwrap();
	let before = fixture.revalidations.get();
	let cancellation = Cancellation::default();
	let first = session
		.query_runs(RunQuery::default(), 0, 1, &cancellation)
		.unwrap();
	assert_eq!(fixture.revalidations.get(), before + 2);
	assert_eq!(first.offset, 0);
	assert_eq!(first.next_offset, expected.next_offset);
	assert_eq!(first.rows[0].ordinal, 0);
	assert_eq!(first.rows[0].run, expected.runs[0]);
	assert!(matches!(
		session.query_runs(RunQuery::default(), 0, 0, &cancellation),
		Err(Error::ResourceLimit(_))
	));
	let beyond = session
		.query_runs(RunQuery::default(), usize::MAX, 1, &cancellation)
		.unwrap();
	assert!(beyond.rows.is_empty());
	assert_eq!(beyond.next_offset, None);
	fixture
		.change_at_check
		.set(Some(fixture.revalidations.get() + 2));
	assert!(matches!(
		session.query_runs(RunQuery::default(), 0, 1, &cancellation),
		Err(Error::SourceChanged)
	));
	cancellation.cancel();
	assert!(matches!(
		session.query_runs(RunQuery::default(), 0, 1, &cancellation),
		Err(Error::Cancelled)
	));
}

#[test]
fn locating_a_run_revalidates_the_image_and_uses_only_the_retained_index() {
	let fixture = Fixture::default();
	let session =
		ImageSession::open(&fixture, Limits::default(), MetadataBudget::ReaderLimits).unwrap();
	fixture.closed_to_reads.set(true);
	let checks = fixture.revalidations.get();
	let location = session.locate_run(0, 1).unwrap();
	assert_eq!(location.ordinal, 0);
	assert_eq!(location.page.runs[0].logical.offset_bytes, 0);
	assert_eq!(fixture.revalidations.get(), checks + 1);
	assert!(matches!(
		session.locate_run(0, 0),
		Err(Error::ResourceLimit(_))
	));
	fixture.changed.set(true);
	assert!(matches!(
		session.locate_run(0, 1),
		Err(Error::SourceChanged)
	));
}
