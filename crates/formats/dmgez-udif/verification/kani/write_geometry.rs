use crate::{binary, trailer};

const WINDOW_BYTES: usize = 64;
const KOLY_BYTES: usize = 512;
const KOLY_HEADER: &[u8] = b"koly\0\0\0\x04\0\0\x02\0";
const KOLY_DATA_LENGTH: usize = 32;
const KOLY_XML_START: usize = 216;
const KOLY_XML_LENGTH: usize = 224;
const KOLY_SECTOR_COUNT: usize = 492;

#[kani::proof]
#[kani::unwind(66)]
fn fixed_writes_change_only_the_requested_window() {
	let bytes: [u8; WINDOW_BYTES] = kani::any();
	let (len, offset): (usize, usize) = kani::any();
	let (short, long): (u32, u64) = kani::any();
	kani::assume(len <= WINDOW_BYTES && offset <= len);
	let mut short_result = bytes;
	let mut long_result = bytes;

	if len - offset >= size_of::<u32>() {
		binary::put_u32(&mut short_result[..len], offset, short);
		check_window(&short_result, &bytes, offset, short.to_be_bytes());
	}
	if len - offset >= size_of::<u64>() {
		binary::put_u64(&mut long_result[..len], offset, long);
		check_window(&long_result, &bytes, offset, long.to_be_bytes());
	}
}

fn check_window<const WIDTH: usize>(
	actual: &[u8; WINDOW_BYTES],
	before: &[u8; WINDOW_BYTES],
	offset: usize,
	value: [u8; WIDTH],
) {
	for (index, &byte) in actual.iter().enumerate() {
		let expected = index
			.checked_sub(offset)
			.and_then(|index| value.get(index))
			.copied()
			.unwrap_or(before[index]);
		assert_eq!(byte, expected);
	}
}

#[kani::proof]
#[kani::unwind(18)]
fn trailer_writes_geometry_at_the_exact_wire_offsets() {
	let (sectors, data_len, xml_len): (u64, u64, u64) = kani::any();
	let (physical_crc, master_crc, variant): (u32, u32, u32) = kani::any();
	let segment_id = kani::any();
	let bytes = trailer::encode(
		sectors,
		data_len,
		xml_len,
		physical_crc,
		master_crc,
		variant,
		segment_id,
	);
	assert_eq!(bytes.len(), KOLY_BYTES);
	assert_eq!(&bytes[..KOLY_HEADER.len()], KOLY_HEADER);
	for (offset, value) in [
		(KOLY_DATA_LENGTH, data_len),
		(KOLY_XML_START, data_len),
		(KOLY_XML_LENGTH, xml_len),
		(KOLY_SECTOR_COUNT, sectors),
	] {
		assert_eq!(bytes[offset..][..size_of::<u64>()], value.to_be_bytes());
	}
}
