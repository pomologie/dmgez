#![no_main]

use dmgez_udif::{
	SECTOR_BYTES,
	reader::{Image, Limits},
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const READ_WINDOW_BYTES: usize = SECTOR_BYTES as usize;
const FUZZ_LIMITS: Limits = Limits {
	max_physical_bytes: MAX_INPUT_BYTES as u64,
	max_logical_bytes: 2 * 1024 * 1024,
	max_xml_bytes: 48 * 1024,
	max_tables: 32,
	max_records: 1024,
	max_decoded_bytes: 1024 * 1024,
	max_compressed_bytes: MAX_INPUT_BYTES,
};

fuzz_target!(|bytes: &[u8]| {
	if let Ok(mut image) = Image::open(bytes, FUZZ_LIMITS) {
		let midpoint = image.descriptor().logical_bytes / 2;
		let offset = midpoint.saturating_sub(SECTOR_BYTES);
		// Reads and verification may reject mutated inputs.
		let _ = image.read_exact_at(offset, &mut [0; READ_WINDOW_BYTES]);
		let _ = image.verify();
	}
});
