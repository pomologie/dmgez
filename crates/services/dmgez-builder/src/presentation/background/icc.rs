use std::mem::size_of;

// ICC.1:2022 sections 7.2–7.4 describe the header and tag table.
const HEADER_BYTES: usize = 128;
const SIZE_OFFSET: usize = 0;
const COLOR_SPACE_OFFSET: usize = 16;
const SIGNATURE_OFFSET: usize = 36;
const PROFILE_SIGNATURE: [u8; 4] = *b"acsp";
const GRAY_SPACE: [u8; 4] = *b"GRAY";
const RGB_SPACE: [u8; 4] = *b"RGB ";
const TAG_COUNT_OFFSET: usize = HEADER_BYTES;
const TAG_TABLE_OFFSET: usize = TAG_COUNT_OFFSET + size_of::<u32>();
const TAG_ENTRY_BYTES: usize = 12;
const TAG_DATA_OFFSET: usize = 4;
const TAG_DATA_SIZE_OFFSET: usize = 8;
const TAG_DATA_ALIGNMENT: usize = 4;
const TAG_DATA_HEADER_BYTES: usize = 8;

/// Checks the profile's declared byte ranges and color model before copying it.
/// Each tag payload is preserved as opaque data.
pub(super) fn valid_for_model(profile: &[u8], gray: bool) -> bool {
	let expected = if gray { GRAY_SPACE } else { RGB_SPACE };
	if u32_at(profile, SIZE_OFFSET) != Some(profile.len())
		|| profile.get(SIGNATURE_OFFSET..SIGNATURE_OFFSET + PROFILE_SIGNATURE.len())
			!= Some(&PROFILE_SIGNATURE)
		|| profile.get(COLOR_SPACE_OFFSET..COLOR_SPACE_OFFSET + expected.len()) != Some(&expected)
	{
		return false;
	}
	let Some(table_end) = u32_at(profile, TAG_COUNT_OFFSET)
		.filter(|&count| count > 0)
		.and_then(|count| count.checked_mul(TAG_ENTRY_BYTES))
		.and_then(|bytes| TAG_TABLE_OFFSET.checked_add(bytes))
	else {
		return false;
	};
	let Some(table) = profile.get(TAG_TABLE_OFFSET..table_end) else {
		return false;
	};
	table.as_chunks::<TAG_ENTRY_BYTES>().0.iter().all(|entry| {
		u32_at(entry, TAG_DATA_OFFSET)
			.filter(|&offset| offset >= table_end && offset.is_multiple_of(TAG_DATA_ALIGNMENT))
			.zip(u32_at(entry, TAG_DATA_SIZE_OFFSET).filter(|&size| size >= TAG_DATA_HEADER_BYTES))
			.and_then(|(offset, size)| offset.checked_add(size))
			.is_some_and(|end| end <= profile.len())
	})
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<usize> {
	let value = bytes.get(offset..)?.first_chunk::<{ size_of::<u32>() }>()?;
	usize::try_from(u32::from_be_bytes(*value)).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn one_tag_profile() -> [u8; TAG_TABLE_OFFSET + TAG_ENTRY_BYTES + TAG_DATA_HEADER_BYTES] {
		let mut profile = [0; TAG_TABLE_OFFSET + TAG_ENTRY_BYTES + TAG_DATA_HEADER_BYTES];
		let len = u32::try_from(profile.len()).unwrap();
		let tag_data_offset = u32::try_from(TAG_TABLE_OFFSET + TAG_ENTRY_BYTES).unwrap();
		profile[SIZE_OFFSET..SIZE_OFFSET + size_of::<u32>()].copy_from_slice(&len.to_be_bytes());
		profile[SIGNATURE_OFFSET..SIGNATURE_OFFSET + PROFILE_SIGNATURE.len()]
			.copy_from_slice(&PROFILE_SIGNATURE);
		profile[COLOR_SPACE_OFFSET..COLOR_SPACE_OFFSET + RGB_SPACE.len()]
			.copy_from_slice(&RGB_SPACE);
		profile[TAG_COUNT_OFFSET..TAG_TABLE_OFFSET].copy_from_slice(&1_u32.to_be_bytes());
		profile[TAG_TABLE_OFFSET + TAG_DATA_OFFSET..TAG_TABLE_OFFSET + TAG_DATA_SIZE_OFFSET]
			.copy_from_slice(&tag_data_offset.to_be_bytes());
		profile[TAG_TABLE_OFFSET + TAG_DATA_SIZE_OFFSET..TAG_TABLE_OFFSET + TAG_ENTRY_BYTES]
			.copy_from_slice(&u32::try_from(TAG_DATA_HEADER_BYTES).unwrap().to_be_bytes());
		profile
	}

	#[test]
	fn refuses_truncated_tables_and_tags_pointing_into_the_table_or_outside_the_profile() {
		let profile = one_tag_profile();
		assert!(valid_for_model(&profile, false));
		assert!(!valid_for_model(&profile, true));

		let mut bad_count = profile;
		bad_count[TAG_COUNT_OFFSET..TAG_TABLE_OFFSET].copy_from_slice(&u32::MAX.to_be_bytes());
		assert!(!valid_for_model(&bad_count, false));

		for offset in [0, u32::MAX, u32::try_from(profile.len() - 4).unwrap()] {
			let mut invalid = profile;
			invalid[TAG_TABLE_OFFSET + TAG_DATA_OFFSET..TAG_TABLE_OFFSET + TAG_DATA_SIZE_OFFSET]
				.copy_from_slice(&offset.to_be_bytes());
			assert!(!valid_for_model(&invalid, false));
		}
	}
}
