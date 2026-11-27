use std::mem::size_of;

use tiff::tags::{Tag, Type};

use super::super::TIFF_TOP_LEFT;

const TIFF_MAGIC: u16 = 42;
const TIFF_HEADER_BYTES: usize = 8;
const DIRECTORY_ENTRY_BYTES: usize = 12;
const SINGLE_VALUE: u32 = 1;

/// Checks the primary image's orientation without reading EXIF subdirectories.
pub(super) fn is_top_left(exif: &[u8]) -> bool {
	primary_orientation(exif).is_some_and(|orientation| orientation == TIFF_TOP_LEFT)
}

fn primary_orientation(exif: &[u8]) -> Option<u16> {
	let (order, header) = exif.split_first_chunk::<{ size_of::<u16>() }>()?;
	let (read_short, read_long) = match order {
		b"II" => (
			u16::from_le_bytes as fn([u8; 2]) -> u16,
			u32::from_le_bytes as fn([u8; 4]) -> u32,
		),
		b"MM" => (
			u16::from_be_bytes as fn([u8; 2]) -> u16,
			u32::from_be_bytes as fn([u8; 4]) -> u32,
		),
		_ => return None,
	};
	let (magic, offset) = header.split_first_chunk::<{ size_of::<u16>() }>()?;
	let offset = usize::try_from(read_long(*offset.first_chunk()?)).ok()?;
	if read_short(*magic) != TIFF_MAGIC || offset < TIFF_HEADER_BYTES {
		return None;
	}
	let (count, directory) = exif
		.get(offset..)?
		.split_first_chunk::<{ size_of::<u16>() }>()?;
	let bytes = usize::from(read_short(*count)).checked_mul(DIRECTORY_ENTRY_BYTES)?;
	let (records, next_offset) = directory.split_at_checked(bytes)?;
	next_offset.first_chunk::<{ size_of::<u32>() }>()?;

	let mut orientation = None;
	for &[
		tag_a,
		tag_b,
		type_a,
		type_b,
		count_a,
		count_b,
		count_c,
		count_d,
		value_a,
		value_b,
		_,
		_,
	] in records.as_chunks::<DIRECTORY_ENTRY_BYTES>().0
	{
		if read_short([tag_a, tag_b]) != Tag::Orientation.to_u16() {
			continue;
		}
		if read_short([type_a, type_b]) != Type::SHORT.to_u16()
			|| read_long([count_a, count_b, count_c, count_d]) != SINGLE_VALUE
			|| orientation.is_some()
		{
			return None;
		}
		orientation = Some(read_short([value_a, value_b]));
	}
	Some(orientation.unwrap_or(TIFF_TOP_LEFT))
}
