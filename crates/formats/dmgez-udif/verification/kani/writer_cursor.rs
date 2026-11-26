use std::{
	io::{self, Write},
	mem::ManuallyDrop,
};

use crate::Error;

use super::{image_end, write_payload};

const WIRE_TRAILER_BYTES: u128 = 512;
const WIRE_SECTOR_BYTES: usize = 512;

#[derive(Default)]
struct CountingSink {
	calls: u64,
	bytes: usize,
}

impl Write for CountingSink {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.calls += 1;
		self.bytes += bytes.len();
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[kani::proof]
#[kani::unwind(4)]
fn data_cursor_checks_the_final_sector_before_any_sink_write() {
	let (start, limit): (u64, u64) = kani::any();
	let mut sink = CountingSink::default();
	let end = u128::from(start) + WIRE_SECTOR_BYTES as u128;
	let expected = end + WIRE_TRAILER_BYTES <= u128::from(limit);
	// Inspect the returned variant without pulling unrelated error destructors
	// into the model. The sink never constructs an I/O error.
	let outcome = ManuallyDrop::new(write_payload(
		&mut sink,
		start,
		limit,
		&[0; WIRE_SECTOR_BYTES],
	));

	if expected {
		assert!(matches!(&*outcome, Ok(value) if u128::from(*value) == end));
		assert_eq!(sink.calls, 1);
		assert_eq!(sink.bytes, WIRE_SECTOR_BYTES);
	} else {
		assert!(matches!(&*outcome, Err(Error::ResourceLimit(_))));
		assert_eq!(sink.calls, 0);
		assert_eq!(sink.bytes, 0);
	}
	kani::cover!(end + WIRE_TRAILER_BYTES == u128::from(limit));
	kani::cover!(end > u128::from(u64::MAX));
	kani::cover!(end <= u128::from(limit) && end + WIRE_TRAILER_BYTES > u128::from(limit));
}

#[kani::proof]
#[kani::unwind(4)]
fn metadata_cursor_checks_both_lengths_at_full_width() {
	let (start, xml_len, limit): (u64, u64, u64) = kani::any();
	let end = u128::from(start) + u128::from(xml_len) + WIRE_TRAILER_BYTES;
	let result = ManuallyDrop::new(image_end(start, xml_len, limit));
	if end <= u128::from(limit) {
		assert!(matches!(&*result, Ok(value) if u128::from(*value) == end));
	} else {
		assert!(matches!(&*result, Err(Error::ResourceLimit(_))));
	}
	kani::cover!(end == u128::from(limit));
	kani::cover!(u128::from(xml_len) + WIRE_TRAILER_BYTES > u128::from(u64::MAX));
	kani::cover!(
		u128::from(xml_len) + WIRE_TRAILER_BYTES <= u128::from(u64::MAX)
			&& end > u128::from(u64::MAX)
	);
}
