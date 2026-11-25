#[cfg(feature = "read")]
use std::ops::Range;

#[cfg(feature = "write")]
use crate::trailer_header::write_trailer_header;
use crate::trailer_header::{DATA_FORK_LENGTH_OFFSET, XML_LENGTH_OFFSET, XML_OFFSET};
#[cfg(feature = "read")]
use crate::{
	Error, SECTOR_BYTES, binary, check_trailer_header,
	checksum::Checksum,
	trailer_header::{DATA_FORK_OFFSET, RESOURCE_FORK_LENGTH_OFFSET, RESOURCE_FORK_OFFSET},
	validation::checked_range,
};

pub(crate) use crate::TRAILER_BYTES as BYTES;
const FLAG_FLATTENED: u32 = 1;
const SINGLE_SEGMENT_NUMBER: u32 = 1;
const SINGLE_SEGMENT_COUNT: u32 = 1;
#[cfg(feature = "read")]
const DEVICE_IMAGE: u32 = 1;
pub(crate) const PARTITION_IMAGE: u32 = 2;

const FLAGS_OFFSET: usize = 12;
#[cfg(feature = "read")]
const RUNNING_DATA_FORK_OFFSET: usize = 16;
const SEGMENT_NUMBER_OFFSET: usize = 56;
const SEGMENT_COUNT_OFFSET: usize = 60;
const SEGMENT_ID_OFFSET: usize = 64;
pub(crate) const SEGMENT_ID_BYTES: usize = 16;
const DATA_CHECKSUM_OFFSET: usize = 80;
#[cfg(feature = "read")]
const RESERVED_BEFORE_MASTER_CHECKSUM_OFFSET: usize = 232;
const MASTER_CHECKSUM_OFFSET: usize = 352;
const IMAGE_VARIANT_OFFSET: usize = 488;
const SECTOR_COUNT_OFFSET: usize = 492;
#[cfg(feature = "read")]
const RESERVED_TAIL_OFFSET: usize = 500;

#[cfg(feature = "read")]
#[derive(Debug)]
pub(crate) struct Trailer {
	pub data: Range<u64>,
	pub xml: Range<u64>,
	pub logical_bytes: u64,
	pub segment_id: [u8; SEGMENT_ID_BYTES],
	pub variant: u32,
	pub physical_crc: Checksum,
	pub master_crc: Checksum,
}

#[cfg(all(feature = "read", feature = "write"))]
pub(crate) fn has_plain_checksums(bytes: &[u8; BYTES]) -> bool {
	crate::checksum::is_plain_crc32(bytes, DATA_CHECKSUM_OFFSET)
		&& crate::checksum::is_plain_crc32(bytes, MASTER_CHECKSUM_OFFSET)
}

#[cfg(feature = "read")]
impl Trailer {
	pub fn decode(bytes: &[u8; BYTES], body_len: u64) -> Result<Self, Error> {
		check_trailer_header(bytes)?;
		if binary::u32_at(bytes, FLAGS_OFFSET)? != FLAG_FLATTENED {
			return Err(Error::Unsupported("koly flags"));
		}
		if binary::u32_at(bytes, SEGMENT_NUMBER_OFFSET)? != SINGLE_SEGMENT_NUMBER
			|| binary::u32_at(bytes, SEGMENT_COUNT_OFFSET)? != SINGLE_SEGMENT_COUNT
		{
			return Err(Error::Unsupported("segmented disk image"));
		}
		if binary::u64_at(bytes, RUNNING_DATA_FORK_OFFSET)? != 0 {
			return Err(Error::Unsupported("nonzero running data-fork offset"));
		}
		let data = physical_range(bytes, DATA_FORK_OFFSET, DATA_FORK_LENGTH_OFFSET, body_len)?;
		let xml = physical_range(bytes, XML_OFFSET, XML_LENGTH_OFFSET, body_len)?;
		let resource = physical_range(
			bytes,
			RESOURCE_FORK_OFFSET,
			RESOURCE_FORK_LENGTH_OFFSET,
			body_len,
		)?;
		if !resource.is_empty() || resource.start != 0 || xml.is_empty() {
			return Err(Error::Unsupported("non-XML resource fork or missing plist"));
		}
		if data.start < xml.end && xml.start < data.end {
			return Err(Error::Corrupt("XML overlaps the data fork"));
		}
		if bytes[RESERVED_BEFORE_MASTER_CHECKSUM_OFFSET..MASTER_CHECKSUM_OFFSET]
			.iter()
			.chain(&bytes[RESERVED_TAIL_OFFSET..])
			.any(|&byte| byte != 0)
		{
			return Err(Error::Unsupported("signed or extended koly trailer"));
		}
		let variant = binary::u32_at(bytes, IMAGE_VARIANT_OFFSET)?;
		if !matches!(variant, DEVICE_IMAGE | PARTITION_IMAGE) {
			return Err(Error::Unsupported("koly image variant"));
		}
		let logical_bytes = binary::u64_at(bytes, SECTOR_COUNT_OFFSET)?
			.checked_mul(SECTOR_BYTES)
			.filter(|len| *len != 0)
			.ok_or(Error::Corrupt("invalid logical disk length"))?;
		Ok(Self {
			data,
			xml,
			logical_bytes,
			segment_id: binary::field(bytes, SEGMENT_ID_OFFSET)?,
			variant,
			physical_crc: Checksum::decode(bytes, DATA_CHECKSUM_OFFSET)?,
			master_crc: Checksum::decode(bytes, MASTER_CHECKSUM_OFFSET)?,
		})
	}
}

#[cfg(feature = "read")]
fn physical_range(
	bytes: &[u8],
	start_offset: usize,
	length_offset: usize,
	limit: u64,
) -> Result<Range<u64>, Error> {
	let start = binary::u64_at(bytes, start_offset)?;
	let len = binary::u64_at(bytes, length_offset)?;
	checked_range(start, len, limit).ok_or(Error::Corrupt("koly range outside image body"))
}

#[cfg(feature = "write")]
pub(crate) fn encode(
	sectors: u64,
	data_len: u64,
	xml_len: u64,
	physical_crc: u32,
	master_crc: u32,
	variant: u32,
	segment_id: [u8; SEGMENT_ID_BYTES],
) -> [u8; BYTES] {
	use crate::{binary, checksum};

	let mut bytes = [0; BYTES];
	write_trailer_header(&mut bytes);
	binary::put_u32(&mut bytes, FLAGS_OFFSET, FLAG_FLATTENED);
	binary::put_u64(&mut bytes, DATA_FORK_LENGTH_OFFSET, data_len);
	binary::put_u32(&mut bytes, SEGMENT_NUMBER_OFFSET, SINGLE_SEGMENT_NUMBER);
	binary::put_u32(&mut bytes, SEGMENT_COUNT_OFFSET, SINGLE_SEGMENT_COUNT);
	bytes[SEGMENT_ID_OFFSET..SEGMENT_ID_OFFSET + SEGMENT_ID_BYTES].copy_from_slice(&segment_id);
	checksum::put_crc32(&mut bytes, DATA_CHECKSUM_OFFSET, physical_crc);
	binary::put_u64(&mut bytes, XML_OFFSET, data_len);
	binary::put_u64(&mut bytes, XML_LENGTH_OFFSET, xml_len);
	checksum::put_crc32(&mut bytes, MASTER_CHECKSUM_OFFSET, master_crc);
	binary::put_u32(&mut bytes, IMAGE_VARIANT_OFFSET, variant);
	binary::put_u64(&mut bytes, SECTOR_COUNT_OFFSET, sectors);
	bytes
}
