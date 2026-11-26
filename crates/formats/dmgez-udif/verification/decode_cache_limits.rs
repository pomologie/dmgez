use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;

#[test]
fn rejects_excessive_reservation_and_decoder_capacity() {
	let cache = DecodeCache::new(1);
	let mut output = [0xff];
	assert!(matches!(
		cache.copy(0, 2 + DECODER_SPARE_BYTES, 0, &mut output, || {
			panic!("the reservation must be checked before decode")
		}),
		Err(Error::ResourceLimit("decoded cache bytes"))
	));
	assert!(matches!(
		cache.copy(0, 1, 0, &mut output, || Ok(vec![0; 2])),
		Err(Error::ResourceLimit("decoded cache bytes"))
	));
	assert_eq!(output, [0xff]);

	cache.copy(0, 1, 0, &mut output, || Ok(vec![7])).unwrap();
	assert_eq!(output, [7]);
}

#[test]
fn refuses_invalid_output_windows_without_copying() {
	let cache = DecodeCache::new(1);
	let mut output = [0xff];
	for (offset, expected) in [
		(usize::MAX, "decoded run range overflow"),
		(1, "read outside decoded run"),
	] {
		assert!(matches!(
			cache.copy(0, 1, offset, &mut output, || Ok(vec![7])),
			Err(Error::Corrupt(detail)) if detail == expected
		));
		assert_eq!(output, [0xff]);
	}
}

#[test]
fn a_poisoned_cache_refuses_reads_and_reset() {
	let mut cache = DecodeCache::new(1);
	let mut output = [0xff];
	let panic = catch_unwind(AssertUnwindSafe(|| {
		let _ = cache.copy(0, 1, 0, &mut output, || panic!("injected decoder panic"));
	}));
	assert!(panic.is_err());
	assert!(matches!(
		cache.copy(0, 1, 0, &mut output, || panic!("poisoned cache must refuse decode")),
		Err(Error::Io(error)) if error.to_string() == "UDIF decode cache lock poisoned"
	));
	assert_eq!(output, [0xff]);
	assert!(matches!(cache.clear(), Err(Error::Io(_))));
}
