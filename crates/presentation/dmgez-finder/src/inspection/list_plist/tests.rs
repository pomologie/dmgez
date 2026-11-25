use super::{
	ARRAY_MARKER, BYTE_STRING_MARKER, DATA_MARKER, DICTIONARY_MARKER, FALSE_MARKER, INTEGER_MARKER,
	UTF16_STRING_MARKER, admit,
};
use crate::inspection::{
	ReadBudget, ReadError, ReadLimits,
	binary_plist::{MARKER_VALUE, PLIST_MAGIC, TRAILER_PADDING_BYTES},
};

const OFFSET_BYTES: usize = size_of::<u32>();
const REFERENCE_BYTES: u8 = 1;
const ROOT: u64 = 0;
const STRING_KEY: &[u8] = &[BYTE_STRING_MARKER | 1, b'x'];
const ROOT_OBJECT: &[u8] = &[DICTIONARY_MARKER | 1, 1, 2];
const LARGE_DATA_BYTES: u16 = 20_000;
const REPEATED_ENTRIES: u8 = 64;

fn binary(objects: &[&[u8]]) -> Vec<u8> {
	let mut bytes = PLIST_MAGIC.to_vec();
	let mut addresses = Vec::new();
	for object in objects {
		addresses.push(u32::try_from(bytes.len()).unwrap());
		bytes.extend_from_slice(object);
	}
	let table = u64::try_from(bytes.len()).unwrap();
	for offset in addresses {
		bytes.extend_from_slice(&offset.to_be_bytes());
	}
	bytes.extend_from_slice(&[0; TRAILER_PADDING_BYTES]);
	bytes.push(u8::try_from(OFFSET_BYTES).unwrap());
	bytes.push(REFERENCE_BYTES);
	bytes.extend_from_slice(&u64::try_from(objects.len()).unwrap().to_be_bytes());
	bytes.extend_from_slice(&ROOT.to_be_bytes());
	bytes.extend_from_slice(&table.to_be_bytes());
	bytes
}

fn shared_array(reference: u8) -> Vec<u8> {
	let mut array = vec![
		ARRAY_MARKER | MARKER_VALUE,
		INTEGER_MARKER,
		REPEATED_ENTRIES,
	];
	array.resize(array.len() + usize::from(REPEATED_ENTRIES), reference);
	array
}

#[test]
fn permits_shared_objects_but_rejects_expanded_object_count() {
	let array = shared_array(3);
	let inner = shared_array(4);
	let leaf = &[FALSE_MARKER][..];
	let bytes = binary(&[ROOT_OBJECT, STRING_KEY, &array, leaf]);
	admit(&bytes, ReadLimits::default()).unwrap();
	let bytes = binary(&[ROOT_OBJECT, STRING_KEY, &array, &inner, leaf]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::Limit(ReadBudget::PlistExpansion))
	));
}

#[test]
fn charges_shared_data_bytes_on_each_visit() {
	let array = shared_array(3);
	let mut data = vec![DATA_MARKER | MARKER_VALUE, INTEGER_MARKER | 1];
	data.extend_from_slice(&LARGE_DATA_BYTES.to_be_bytes());
	data.resize(data.len() + usize::from(LARGE_DATA_BYTES), 0);
	let bytes = binary(&[ROOT_OBJECT, STRING_KEY, &array, &data]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::Limit(ReadBudget::PlistExpansion))
	));
}

#[test]
fn rejects_recursive_and_deep_graphs() {
	let cycle = &[ARRAY_MARKER | 1, 2];
	let bytes = binary(&[ROOT_OBJECT, STRING_KEY, cycle]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::PlistLayout)
	));

	let mut objects = vec![ROOT_OBJECT.to_vec(), STRING_KEY.to_vec()];
	for next in 3..=9 {
		objects.push(vec![ARRAY_MARKER | 1, next]);
	}
	objects.push(vec![FALSE_MARKER]);
	let objects = objects.iter().map(Vec::as_slice).collect::<Vec<_>>();
	assert!(matches!(
		admit(&binary(&objects), ReadLimits::default()),
		Err(ReadError::Limit(ReadBudget::PlistDepth))
	));
}

#[test]
fn rejects_duplicate_decoded_dictionary_keys() {
	let nested = &[DICTIONARY_MARKER | 2, 1, 3, 4, 4];
	let unicode_key = &[UTF16_STRING_MARKER | 1, 0, b'x'];
	let bytes = binary(&[
		ROOT_OBJECT,
		STRING_KEY,
		nested,
		unicode_key,
		&[FALSE_MARKER],
	]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::PlistLayout)
	));
}

#[test]
fn rejects_out_of_bounds_references_and_overlapping_objects() {
	let root = &[DICTIONARY_MARKER | 1, 1, 3];
	let bytes = binary(&[root, STRING_KEY, &[FALSE_MARKER]]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::PlistLayout)
	));

	let data_claiming_following_objects = &[DATA_MARKER | 2, 0];
	let bytes = binary(&[
		ROOT_OBJECT,
		STRING_KEY,
		data_claiming_following_objects,
		&[FALSE_MARKER],
	]);
	assert!(matches!(
		admit(&bytes, ReadLimits::default()),
		Err(ReadError::PlistLayout)
	));
}
