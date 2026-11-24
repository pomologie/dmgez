#[cfg(feature = "read")]
use std::ops::Range;

#[cfg(feature = "read")]
use crate::{Error, SECTOR_BYTES, binary, checksum::Checksum, validation::checked_range};

pub(crate) const BYTES: usize = 512;
const SIGNATURE: &[u8; 4] = b"koly";
const VERSION: u32 = 4;
const FLAG_FLATTENED: u32 = 1;
const SINGLE_SEGMENT_NUMBER: u32 = 1;
const SINGLE_SEGMENT_COUNT: u32 = 1;
#[cfg(feature = "read")]
const DEVICE_IMAGE: u32 = 1;
const PARTITION_IMAGE: u32 = 2;

const VERSION_OFFSET: usize = 4;
const HEADER_LENGTH_OFFSET: usize = 8;
const FLAGS_OFFSET: usize = 12;
#[cfg(feature = "read")]
const RUNNING_DATA_FORK_OFFSET: usize = 16;
#[cfg(feature = "read")]
const DATA_FORK_OFFSET: usize = 24;
const DATA_FORK_LENGTH_OFFSET: usize = 32;
#[cfg(feature = "read")]
const RESOURCE_FORK_OFFSET: usize = 40;
#[cfg(feature = "read")]
const RESOURCE_FORK_LENGTH_OFFSET: usize = 48;
const SEGMENT_NUMBER_OFFSET: usize = 56;
const SEGMENT_COUNT_OFFSET: usize = 60;
#[cfg(feature = "read")]
const SEGMENT_ID_OFFSET: usize = 64;
#[cfg(feature = "read")]
const SEGMENT_ID_BYTES: usize = 16;
const DATA_CHECKSUM_OFFSET: usize = 80;
const XML_OFFSET: usize = 216;
const XML_LENGTH_OFFSET: usize = 224;
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

#[cfg(feature = "read")]
impl Trailer {
	pub fn decode(bytes: &[u8; BYTES], body_len: u64) -> Result<Self, Error> {
		if &bytes[..SIGNATURE.len()] != SIGNATURE {
			return Err(Error::InvalidFormat("missing koly trailer"));
		}
		if binary::u32_at(bytes, VERSION_OFFSET)? != VERSION {
			return Err(Error::Unsupported("koly version"));
		}
		if binary::u32_at(bytes, HEADER_LENGTH_OFFSET)? != BYTES as u32 {
			return Err(Error::Corrupt("koly header length"));
		}
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
