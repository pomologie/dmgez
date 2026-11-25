use crate::{
	binary::{field, u32_at, u64_at},
	blkx::{admit_records, encoded_len},
	trailer::Trailer,
};

// Wire positions and sizes from the fixture format, independent of the codec.
const MISH_HEADER: usize = 204;
const MISH_RECORD: usize = 40;
const MISH_COUNT: usize = 200;
const MISH_PREFIX: &[u8] = b"mish\0\0\0\x01";
const MAX_RECORDS: usize = 8;
const WINDOW_BYTES: usize = 64;
const KOLY_BYTES: usize = 512;
const KOLY_DATA_START: usize = 24;
const KOLY_DATA_LENGTH: usize = 32;
const KOLY_XML_START: usize = 216;
const KOLY_XML_LENGTH: usize = 224;
const KOLY_SECTOR_COUNT: usize = 492;
const DISK_SECTOR_BYTES: u128 = 512;
const NATIVE_IMAGE: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDRO.dmg");

#[kani::proof]
#[kani::unwind(18)]
fn fixed_fields_match_bytewise_windows() {
	let bytes: [u8; WINDOW_BYTES] = kani::any();
	let len: usize = kani::any();
	kani::assume(len <= WINDOW_BYTES);
	let window = &bytes[..len];
	let offset: usize = kani::any();

	let decode = |width| {
		let end = offset as u128 + width as u128;
		(end <= len as u128).then(|| {
			(offset..end as usize).fold(0u64, |word, index| {
				(word << u8::BITS) | u64::from(bytes[index])
			})
		})
	};
	assert_eq!(
		u32_at(window, offset).ok().map(u64::from),
		decode(size_of::<u32>())
	);
	assert_eq!(u64_at(window, offset).ok(), decode(size_of::<u64>()));

	const UUID_BYTES: usize = 16;
	let segment = field::<UUID_BYTES>(window, offset);
	assert_eq!(
		segment.is_ok(),
		offset as u128 + UUID_BYTES as u128 <= len as u128
	);
	if let Ok(segment) = segment {
		for (index, byte) in segment.into_iter().enumerate() {
			assert_eq!(byte, bytes[offset + index]);
		}
	}
	kani::cover!(segment.is_ok());
	kani::cover!(u32_at(window, offset).is_ok() && u64_at(window, offset).is_err());
	kani::cover!(offset == usize::MAX && len == WINDOW_BYTES);
}

#[kani::proof]
fn block_map_size_matches_widened_geometry() {
	let count: usize = kani::any();
	let expected = MISH_HEADER as u128 + count as u128 * MISH_RECORD as u128;
	assert_eq!(encoded_len(count), usize::try_from(expected).ok());
}

#[kani::proof]
#[kani::unwind(10)]
fn block_map_requires_exact_length_and_budget() {
	let mut bytes = [0; MISH_HEADER + MAX_RECORDS * MISH_RECORD + 1];
	bytes[..MISH_PREFIX.len()].copy_from_slice(MISH_PREFIX);
	let (count, budget, len): (u32, usize, usize) = kani::any();
	kani::assume(budget <= MAX_RECORDS && len <= bytes.len());
	bytes[MISH_COUNT..MISH_COUNT + size_of::<u32>()].copy_from_slice(&count.to_be_bytes());

	let expected = count != 0
		&& count as usize <= budget
		&& len as u128 == MISH_HEADER as u128 + u128::from(count) * MISH_RECORD as u128;
	let actual = admit_records(&bytes[..len], budget);
	assert_eq!(actual.is_ok(), expected);
	if let Ok(records) = actual {
		assert_eq!(records.len(), count as usize);
		assert!(std::ptr::eq(
			records.as_flattened(),
			&bytes[MISH_HEADER..len]
		));
	}
	kani::cover!(expected && count as usize == MAX_RECORDS);
	kani::cover!(count == u32::MAX && len == bytes.len());
}

#[kani::proof]
#[kani::unwind(134)]
fn trailer_matches_widened_endpoints_and_sector_length() {
	let mut bytes = *NATIVE_IMAGE.last_chunk::<KOLY_BYTES>().unwrap();
	let (data_start, data_len, xml_start, xml_len, sectors, body_len): (
		u64,
		u64,
		u64,
		u64,
		u64,
		u64,
	) = kani::any();
	for (offset, value) in [
		(KOLY_DATA_START, data_start),
		(KOLY_DATA_LENGTH, data_len),
		(KOLY_XML_START, xml_start),
		(KOLY_XML_LENGTH, xml_len),
		(KOLY_SECTOR_COUNT, sectors),
	] {
		bytes[offset..offset + size_of::<u64>()].copy_from_slice(&value.to_be_bytes());
	}

	let data_end = u128::from(data_start) + u128::from(data_len);
	let xml_end = u128::from(xml_start) + u128::from(xml_len);
	let logical_len = u128::from(sectors) * DISK_SECTOR_BYTES;
	let expected = data_end <= u128::from(body_len)
		&& xml_end <= u128::from(body_len)
		&& xml_len != 0
		&& (data_end <= u128::from(xml_start) || xml_end <= u128::from(data_start))
		&& (1..=u128::from(u64::MAX)).contains(&logical_len);
	let actual = Trailer::decode(&bytes, body_len);
	assert_eq!(actual.is_ok(), expected);
	if let Ok(trailer) = actual {
		assert_eq!(trailer.data.start, data_start);
		assert_eq!(u128::from(trailer.data.end), data_end);
		assert_eq!(trailer.xml.start, xml_start);
		assert_eq!(u128::from(trailer.xml.end), xml_end);
		assert_eq!(u128::from(trailer.logical_bytes), logical_len);
	}
	kani::cover!(expected && data_end == u128::from(xml_start));
	kani::cover!(data_end > u128::from(u64::MAX) && data_start <= body_len);
	kani::cover!(logical_len > u128::from(u64::MAX));
}
