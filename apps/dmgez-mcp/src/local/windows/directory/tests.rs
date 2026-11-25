use super::{
	ATTRIBUTES_OFFSET, ID_BYTES, ID_OFFSET, NAME_LENGTH_OFFSET, NAME_OFFSET, NEXT_OFFSET,
	RECORD_ALIGNMENT, parse_entry,
};

const TEST_NAME: u16 = b'x' as u16;
const TEST_ID: [u8; ID_BYTES] = [42; ID_BYTES];
const TEST_ATTRIBUTES: u32 = windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
const RECORD_LENGTH: usize = (NAME_OFFSET + size_of::<u16>()).next_multiple_of(RECORD_ALIGNMENT);

fn record(bytes: &mut [u8], next_delta: u32, name_length: u32) {
	bytes[NEXT_OFFSET..NEXT_OFFSET + size_of::<u32>()].copy_from_slice(&next_delta.to_le_bytes());
	bytes[NAME_LENGTH_OFFSET..NAME_LENGTH_OFFSET + size_of::<u32>()]
		.copy_from_slice(&name_length.to_le_bytes());
	bytes[ID_OFFSET..ID_OFFSET + TEST_ID.len()].copy_from_slice(&TEST_ID);
	bytes[ATTRIBUTES_OFFSET..ATTRIBUTES_OFFSET + size_of::<u32>()]
		.copy_from_slice(&TEST_ATTRIBUTES.to_le_bytes());
	bytes[NAME_OFFSET..NAME_OFFSET + size_of::<u16>()].copy_from_slice(&TEST_NAME.to_le_bytes());
}

#[test]
fn binds_names_to_the_record_id_and_validates_batch_boundaries() {
	let unit_bytes = size_of::<u16>() as u32;
	let mut bytes = [0; RECORD_LENGTH * 2];
	record(&mut bytes, RECORD_LENGTH as u32, unit_bytes);
	record(&mut bytes[RECORD_LENGTH..], 0, unit_bytes);
	let (entry, next) = parse_entry(&bytes, 0).unwrap();
	assert_eq!(entry.name(), "x");
	assert_eq!(entry.id, TEST_ID);
	assert_eq!(entry.attributes, TEST_ATTRIBUTES);
	assert_eq!(next, Some(RECORD_LENGTH));
	assert_eq!(parse_entry(&bytes, RECORD_LENGTH).unwrap().1, None);
	for (next, length) in [
		(RECORD_LENGTH as u32 - 1, unit_bytes),
		(RECORD_LENGTH as u32 * 2, unit_bytes),
		(0, unit_bytes + 1),
		(0, u32::MAX - 1),
	] {
		record(&mut bytes, next, length);
		let Err(error) = parse_entry(&bytes, 0) else {
			panic!("accepted inconsistent directory bounds");
		};
		assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
	}
}
