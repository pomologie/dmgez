use proptest::prelude::*;

use crate::{
	Error, TRAILER_BYTES,
	binary::{field, u32_at, u64_at},
	blkx::{Table, encoded_len},
	trailer::Trailer,
};

// Wire positions from the UDIF fixture ledger, independent of the codec's constants.
const MISH_HEADER: usize = 204;
const MISH_RECORD: usize = 40;
const MISH_COUNT: usize = 200;
const MAX_RECORDS: usize = 8;
const WINDOW_BYTES: usize = 64;
const KOLY_DATA_START: usize = 24;
const KOLY_DATA_LENGTH: usize = 32;
const KOLY_XML_START: usize = 216;
const KOLY_XML_LENGTH: usize = 224;
const KOLY_SECTOR_COUNT: usize = 492;
const DISK_SECTOR_BYTES: u128 = 512;
const MAX_SECTOR_COUNT: u64 = u64::MAX / DISK_SECTOR_BYTES as u64;
const NATIVE_TRAILER: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");

fn boundary_u64() -> impl Strategy<Value = u64> {
	prop_oneof![
		any::<u64>(),
		0u64..=128,
		(u64::MAX - 128)..=u64::MAX,
		Just(MAX_SECTOR_COUNT),
		Just(MAX_SECTOR_COUNT + 1),
	]
}

fn write_word(bytes: &mut [u8], offset: usize, value: u64) {
	bytes[offset..offset + size_of::<u64>()].copy_from_slice(&value.to_be_bytes());
}

proptest! {
	#[test]
	fn fixed_width_fields_agree_with_bytewise_decoding(
		bytes in prop::collection::vec(any::<u8>(), 0..=WINDOW_BYTES),
		offset in prop_oneof![0..=WINDOW_BYTES + 1, any::<usize>()],
	) {
		let tail = bytes.iter().copied().skip(offset);
		let decode = |width| {
			let window: Vec<u8> = tail.clone().take(width).collect();
			(window.len() == width).then(|| {
				window.into_iter().fold(0u64, |value, byte| (value << 8) | u64::from(byte))
			})
		};
		prop_assert_eq!(u32_at(&bytes, offset).ok().map(u64::from), decode(size_of::<u32>()));
		prop_assert_eq!(u64_at(&bytes, offset).ok(), decode(size_of::<u64>()));
		let segment: Vec<u8> = tail.take(16).collect();
		prop_assert_eq!(field::<16>(&bytes, offset).ok(), <[u8; 16]>::try_from(segment).ok());
	}

	#[test]
	fn block_map_length_matches_widened_count_arithmetic(
		count in prop_oneof![any::<usize>(), 0..=MAX_RECORDS, Just(usize::MAX)],
	) {
		let expected = MISH_HEADER as u128 + count as u128 * MISH_RECORD as u128;
		prop_assert_eq!(encoded_len(count), usize::try_from(expected).ok());
	}

	#[test]
	fn trailer_geometry_matches_widened_endpoints(
		(data_start, data_length) in (boundary_u64(), boundary_u64()),
		(xml_start, xml_length) in (boundary_u64(), boundary_u64()),
		sectors in boundary_u64(),
		body_length in boundary_u64(),
	) {
		let mut trailer = *NATIVE_TRAILER.last_chunk::<TRAILER_BYTES>().unwrap();
		write_word(&mut trailer, KOLY_DATA_START, data_start);
		write_word(&mut trailer, KOLY_DATA_LENGTH, data_length);
		write_word(&mut trailer, KOLY_XML_START, xml_start);
		write_word(&mut trailer, KOLY_XML_LENGTH, xml_length);
		write_word(&mut trailer, KOLY_SECTOR_COUNT, sectors);

		let data_end = u128::from(data_start) + u128::from(data_length);
		let xml_end = u128::from(xml_start) + u128::from(xml_length);
		let logical_length = u128::from(sectors) * DISK_SECTOR_BYTES;
		let expected = data_end <= u128::from(body_length)
			&& xml_end <= u128::from(body_length)
			&& xml_length != 0
			&& (data_end <= u128::from(xml_start) || xml_end <= u128::from(data_start))
			&& (1..=u128::from(u64::MAX)).contains(&logical_length);
		let actual = Trailer::decode(&trailer, body_length);
		prop_assert_eq!(actual.is_ok(), expected, "{:?}", actual);
		if let Ok(actual) = actual {
			prop_assert_eq!(actual.data.start, data_start);
			prop_assert_eq!(u128::from(actual.data.end), data_end);
			prop_assert_eq!(actual.xml.start, xml_start);
			prop_assert_eq!(u128::from(actual.xml.end), xml_end);
			prop_assert_eq!(u128::from(actual.logical_bytes), logical_length);
		}
	}
}

#[test]
fn block_map_admission_requires_exact_record_bytes_and_budget() {
	const FULL_LENGTH: usize = MISH_HEADER + MISH_RECORD * MAX_RECORDS;
	let mut table = [0; FULL_LENGTH + 1];
	table[..8].copy_from_slice(b"mish\0\0\0\x01");
	for count in 0..=MAX_RECORDS {
		table[MISH_COUNT..MISH_COUNT + size_of::<u32>()]
			.copy_from_slice(&(count as u32).to_be_bytes());
		for length in 0..=table.len() {
			for limit in 0..=MAX_RECORDS {
				let admitted =
					count != 0 && length == MISH_HEADER + count * MISH_RECORD && count <= limit;
				let actual = Table::decode(&table[..length], limit);
				assert_eq!(
					actual.is_ok(),
					admitted,
					"{count} records, {length} bytes, limit {limit}"
				);
				if let Ok(actual) = actual {
					assert_eq!(actual.records.len(), count);
				} else if length >= MISH_HEADER && count > limit {
					assert!(matches!(actual, Err(Error::ResourceLimit("run records"))));
				}
			}
		}
	}
	table[MISH_COUNT..MISH_HEADER].fill(0xff);
	assert!(matches!(
		Table::decode(&table, MAX_RECORDS),
		Err(Error::ResourceLimit("run records"))
	));
	assert!(matches!(
		Table::decode(&table, usize::MAX),
		Err(Error::Corrupt(_))
	));
}

#[test]
fn trailer_rejects_arithmetic_that_would_wrap_into_the_admitted_range() {
	let mut trailer = *NATIVE_TRAILER.last_chunk::<TRAILER_BYTES>().unwrap();
	let body_length = NATIVE_TRAILER.len() as u64 - TRAILER_BYTES as u64;
	write_word(&mut trailer, KOLY_SECTOR_COUNT, MAX_SECTOR_COUNT);
	assert_eq!(
		Trailer::decode(&trailer, body_length)
			.unwrap()
			.logical_bytes,
		MAX_SECTOR_COUNT * DISK_SECTOR_BYTES as u64,
	);
	// One sector beyond the first overflowing count would wrap to a nonzero length.
	write_word(&mut trailer, KOLY_SECTOR_COUNT, MAX_SECTOR_COUNT + 2);
	assert!(matches!(
		Trailer::decode(&trailer, body_length),
		Err(Error::Corrupt("invalid logical disk length")),
	));

	write_word(&mut trailer, KOLY_SECTOR_COUNT, 1);
	write_word(&mut trailer, KOLY_DATA_START, u64::MAX);
	write_word(&mut trailer, KOLY_DATA_LENGTH, 2);
	assert!(matches!(
		Trailer::decode(&trailer, body_length),
		Err(Error::Corrupt("koly range outside image body")),
	));
}
