use std::cell::Cell;

use super::*;

const UDZO: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");

#[derive(Default)]
struct Fixture {
	closed_to_reads: Cell<bool>,
	revalidations: Cell<usize>,
	changed: Cell<bool>,
}

impl ReadAt for &Fixture {
	fn len(&self) -> u64 {
		UDZO.len() as u64
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.revalidations.set(self.revalidations.get() + 1);
		if self.changed.get() {
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
