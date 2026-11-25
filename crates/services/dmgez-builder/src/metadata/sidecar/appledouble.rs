//! The RFC 1740 v2 subset and the macOS `ditto` attribute layout.

use std::ops::Range;

use super::{Error, subtract};
use crate::metadata::FINDER_INFO_BYTES;

const MAGIC: u32 = 0x0005_1607;
const VERSION: u32 = 0x0002_0000;
const MAGIC_FIELD: usize = 0;
const VERSION_FIELD: usize = 4;
const RESERVED_FIELD: Range<usize> = 8..24;
const MAC_OS_X_HEADER: &[u8; 16] = b"Mac OS X        ";
const ENTRY_COUNT_FIELD: usize = 24;
const HEADER_BYTES: usize = 26;
const ENTRY_BYTES: usize = 12;
const ENTRY_ID_FIELD: usize = 0;
const ENTRY_OFFSET_FIELD: usize = 4;
const ENTRY_LENGTH_FIELD: usize = 8;
const RESOURCE_FORK_ID: u32 = 2;
const FINDER_INFO_ID: u32 = 9;
const MAX_ENTRIES: usize = 2;

const FINDER_INFO_PAD_BYTES: usize = 2;
const ATTR_MAGIC: &[u8; 4] = b"ATTR";
const ATTR_HEADER_BYTES: usize = 36;
const ATTR_DEBUG: Range<usize> = 4..8;
const ATTR_TOTAL_FIELD: usize = 8;
const ATTR_DATA_OFFSET_FIELD: usize = 12;
const ATTR_DATA_LENGTH_FIELD: usize = 16;
const ATTR_RESERVED: Range<usize> = 20..34;
const ATTR_COUNT_FIELD: usize = 34;
const ATTR_VALUE_OFFSET_FIELD: usize = 0;
const ATTR_VALUE_LENGTH_FIELD: usize = 4;
const ATTR_FLAGS_FIELD: usize = 8;
const ATTR_NAME_LENGTH_FIELD: usize = 10;
const ATTR_ENTRY_PREFIX_BYTES: usize = 11;
const ATTR_ENTRY_ALIGNMENT: usize = 4;
const MAX_ATTRS: usize = 2_048;
const FINDER_INFO_NAME: &[u8] = b"com.apple.FinderInfo";
const RESOURCE_FORK_NAME: &[u8] = b"com.apple.ResourceFork";
const ACL_NAME: &[u8] = b"com.apple.system.Security";
const COMPRESSION_NAME: &[u8] = b"com.apple.decmpfs";
const NAME_TERMINATOR: u8 = 0;

#[derive(Clone, Copy)]
pub(super) struct Attribute<'a> {
	pub(super) name: &'a [u8],
	pub(super) value: &'a [u8],
}

pub(super) struct Metadata<'a> {
	pub(super) finder_info: Option<[u8; FINDER_INFO_BYTES]>,
	pub(super) resource_fork: Option<&'a [u8]>,
	pub(super) extended: Vec<Attribute<'a>>,
}

impl Metadata<'_> {
	pub(super) fn descriptor_bytes(&self) -> usize {
		self.extended.capacity() * size_of::<Attribute<'_>>()
	}
}

pub(super) fn parse(bytes: &[u8], budget: usize) -> Result<Metadata<'_>, Error> {
	if word(bytes, MAGIC_FIELD)? != MAGIC || word(bytes, VERSION_FIELD)? != VERSION {
		return Err(Error::Unsupported("header or version"));
	}
	let reserved = bytes
		.get(RESERVED_FIELD)
		.ok_or(Error::Malformed("header"))?;
	if !is_zero(reserved) && reserved != MAC_OS_X_HEADER {
		return Err(Error::Unsupported("reserved header data"));
	}
	let count = usize::from(half(bytes, ENTRY_COUNT_FIELD)?);
	if !(1..=MAX_ENTRIES).contains(&count) {
		return Err(Error::Unsupported("record count"));
	}
	let table_end = HEADER_BYTES + ENTRY_BYTES * count;
	let table = bytes
		.get(HEADER_BYTES..table_end)
		.ok_or(Error::Malformed("record table"))?;
	let mut finder = None;
	let mut fork = None;
	for entry in table.as_chunks::<ENTRY_BYTES>().0 {
		let slot = match word(entry, ENTRY_ID_FIELD)? {
			FINDER_INFO_ID => &mut finder,
			RESOURCE_FORK_ID => &mut fork,
			_ => return Err(Error::Unsupported("record type")),
		};
		if slot.is_some() {
			return Err(Error::Malformed("duplicate record"));
		}
		let region = region(entry, ENTRY_OFFSET_FIELD, ENTRY_LENGTH_FIELD, bytes.len())?;
		if region.start < table_end {
			return Err(Error::Malformed("record overlaps its table"));
		}
		*slot = Some(region);
	}
	let mut records = [finder.as_ref(), fork.as_ref()];
	records.sort_unstable_by_key(|record| record.map(|range| (range.start, range.end)));
	let mut cursor = table_end;
	for record in records.into_iter().flatten() {
		verify_gap(bytes, cursor..record.start)?;
		cursor = record.end;
	}
	verify_gap(bytes, cursor..bytes.len())?;
	let resource_fork = fork
		.filter(|region| !region.is_empty())
		.map(|region| &bytes[region]);
	let mut metadata = Metadata {
		finder_info: None,
		resource_fork,
		extended: Vec::new(),
	};
	if let Some(finder) = finder {
		let header = bytes[finder.clone()]
			.first_chunk::<FINDER_INFO_BYTES>()
			.ok_or(Error::Malformed("Finder information"))?;
		metadata.finder_info = (!is_zero(header)).then_some(*header);
		if finder.len() > FINDER_INFO_BYTES {
			metadata.extended = parse_attrs(bytes, finder, budget)?;
		}
	}
	Ok(metadata)
}

fn parse_attrs(
	bytes: &[u8],
	finder: Range<usize>,
	budget: usize,
) -> Result<Vec<Attribute<'_>>, Error> {
	let extension = finder.start + FINDER_INFO_BYTES;
	let start = extension
		.checked_add(FINDER_INFO_PAD_BYTES)
		.ok_or(Error::Malformed("ATTR range"))?;
	verify_gap(bytes, extension..start)?;
	let header_end = start
		.checked_add(ATTR_HEADER_BYTES)
		.ok_or(Error::Malformed("ATTR range"))?;
	let header = bytes
		.get(start..header_end)
		.filter(|_| header_end <= finder.end)
		.ok_or(Error::Malformed("ATTR header"))?;
	// The macOS extension uses absolute AppleDouble file offsets, even for attributes.
	if !header.starts_with(ATTR_MAGIC)
		|| !is_zero(&header[ATTR_DEBUG])
		|| !is_zero(&header[ATTR_RESERVED])
	{
		return Err(Error::Unsupported("ATTR header"));
	}
	let total = size(header, ATTR_TOTAL_FIELD)?;
	let data = region(
		header,
		ATTR_DATA_OFFSET_FIELD,
		ATTR_DATA_LENGTH_FIELD,
		finder.end,
	)?;
	if data.start < header_end || data.end != total || total != finder.end {
		return Err(Error::Malformed("ATTR data area"));
	}
	let count = usize::from(half(header, ATTR_COUNT_FIELD)?);
	if count > MAX_ATTRS || count > (data.start - header_end) / ATTR_ENTRY_PREFIX_BYTES {
		return Err(Error::Malformed("ATTR record count"));
	}
	subtract(budget, count * size_of::<Attribute<'_>>())?;
	let mut result = Vec::new();
	result.try_reserve_exact(count)?;
	let mut entry_at = header_end;
	let mut value_at = data.start;
	for _ in 0..count {
		let tail = bytes
			.get(entry_at..data.start)
			.ok_or(Error::Malformed("ATTR entry"))?;
		let (attribute, next, value_end) = attribute(bytes, tail, entry_at, value_at, data.end)?;
		result.push(attribute);
		entry_at = next;
		value_at = value_end;
	}
	verify_gap(bytes, entry_at..data.start)?;
	if value_at != data.end {
		return Err(Error::Malformed("ATTR data size"));
	}
	result.sort_unstable_by(|a, b| a.name.cmp(b.name));
	if result.windows(2).any(|pair| pair[0].name == pair[1].name) {
		return Err(Error::Malformed("duplicate attribute name"));
	}
	Ok(result)
}

fn attribute<'a>(
	bytes: &'a [u8],
	tail: &'a [u8],
	entry_at: usize,
	value_at: usize,
	data_end: usize,
) -> Result<(Attribute<'a>, usize, usize), Error> {
	let value = region(
		tail,
		ATTR_VALUE_OFFSET_FIELD,
		ATTR_VALUE_LENGTH_FIELD,
		data_end,
	)?;
	if value.start != value_at {
		return Err(Error::Unsupported("non-contiguous attribute values"));
	}
	if half(tail, ATTR_FLAGS_FIELD)? != 0 {
		return Err(Error::Unsupported("attribute flags"));
	}
	let name_end = ATTR_ENTRY_PREFIX_BYTES
		+ usize::from(
			*tail
				.get(ATTR_NAME_LENGTH_FIELD)
				.ok_or(Error::Malformed("attribute name length"))?,
		);
	let name = tail
		.get(ATTR_ENTRY_PREFIX_BYTES..name_end)
		.and_then(|name| name.strip_suffix(&[NAME_TERMINATOR]))
		.filter(|name| !name.is_empty() && !name.contains(&NAME_TERMINATOR))
		.ok_or(Error::Malformed("attribute name"))?;
	if [
		FINDER_INFO_NAME,
		RESOURCE_FORK_NAME,
		ACL_NAME,
		COMPRESSION_NAME,
	]
	.contains(&name)
	{
		return Err(Error::Unsupported("reserved attribute name"));
	}
	let next = name_end.next_multiple_of(ATTR_ENTRY_ALIGNMENT);
	verify_gap(tail, name_end..next)?;
	let attribute = Attribute {
		name,
		value: &bytes[value.clone()],
	};
	Ok((attribute, entry_at + next, value.end))
}

fn verify_gap(bytes: &[u8], range: Range<usize>) -> Result<(), Error> {
	if !bytes.get(range).is_some_and(is_zero) {
		return Err(Error::Malformed("overlapping regions or nonzero padding"));
	}
	Ok(())
}

fn is_zero(bytes: &[u8]) -> bool {
	bytes.iter().all(|&byte| byte == 0)
}

fn region(
	bytes: &[u8],
	offset_field: usize,
	length_field: usize,
	limit: usize,
) -> Result<Range<usize>, Error> {
	let start = size(bytes, offset_field)?;
	let length = size(bytes, length_field)?;
	let end = start
		.checked_add(length)
		.filter(|&end| end <= limit)
		.ok_or(Error::Malformed("record range"))?;
	Ok(start..end)
}

fn size(bytes: &[u8], field: usize) -> Result<usize, Error> {
	usize::try_from(word(bytes, field)?).map_err(|_| Error::Malformed("unrepresentable range"))
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, Error> {
	Ok(u32::from_be_bytes(field(bytes, offset)?))
}

fn half(bytes: &[u8], offset: usize) -> Result<u16, Error> {
	Ok(u16::from_be_bytes(field(bytes, offset)?))
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], Error> {
	bytes
		.get(offset..)
		.and_then(<[u8]>::first_chunk::<N>)
		.copied()
		.ok_or(Error::Malformed("truncated field"))
}
