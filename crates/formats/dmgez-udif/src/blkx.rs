#[cfg(feature = "read")]
use crate::checksum::Checksum;
use crate::{Error, binary};

const SIGNATURE: &[u8; 4] = b"mish";
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 204;
const VERSION_OFFSET: usize = 4;
#[cfg(feature = "read")]
const FIRST_SECTOR_OFFSET: usize = 8;
const SECTOR_COUNT_OFFSET: usize = 16;
#[cfg(feature = "read")]
const DATA_BASE_OFFSET: usize = 24;
const BUFFER_SECTORS_OFFSET: usize = 32;
const DESCRIPTOR_OFFSET: usize = 36;
#[cfg(feature = "read")]
const RESERVED_START: usize = 40;
const CHECKSUM_OFFSET: usize = 64;
const RUN_COUNT_OFFSET: usize = 200;

const RUN_BYTES: usize = 40;
const RUN_TAG_OFFSET: usize = 0;
const RUN_COMMENT_OFFSET: usize = 4;
const RUN_FIRST_SECTOR_OFFSET: usize = 8;
const RUN_SECTOR_COUNT_OFFSET: usize = 16;
const RUN_DATA_OFFSET: usize = 24;
const RUN_STORED_LEN_OFFSET: usize = 32;

const RUN_TAG_ZERO: u32 = 0;
const RUN_TAG_RAW: u32 = 1;
#[cfg(feature = "read")]
const RUN_TAG_IGNORE: u32 = 2;
const RUN_TAG_ZLIB: u32 = 0x8000_0005;
#[cfg(feature = "read")]
const RUN_TAG_COMMENT: u32 = 0x7fff_fffe;
const RUN_TAG_END: u32 = 0xffff_ffff;

pub(crate) const NO_RUN_COMMENT: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
	Raw,
	Zero,
	#[cfg(feature = "read")]
	Ignore,
	Zlib,
	#[cfg(feature = "read")]
	Comment,
	End,
	#[cfg(feature = "read")]
	Unsupported(u32),
}

impl Kind {
	#[cfg(feature = "read")]
	fn decode(code: u32) -> Self {
		match code {
			RUN_TAG_ZERO => Self::Zero,
			RUN_TAG_RAW => Self::Raw,
			RUN_TAG_IGNORE => Self::Ignore,
			RUN_TAG_ZLIB => Self::Zlib,
			RUN_TAG_COMMENT => Self::Comment,
			RUN_TAG_END => Self::End,
			other => Self::Unsupported(other),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Record {
	pub kind: Kind,
	pub comment: u32,
	pub first_sector: u64,
	pub sector_count: u64,
	pub offset: u64,
	pub stored_len: u64,
}

#[cfg(feature = "read")]
#[derive(Debug)]
pub(crate) struct Table {
	pub first_sector: u64,
	pub sector_count: u64,
	pub base: u64,
	pub checksum: Checksum,
	pub buffer_sectors: u32,
	pub descriptor: u32,
	pub records: Vec<Record>,
}

#[cfg(feature = "read")]
impl Table {
	pub fn decode(bytes: &[u8], run_limit: usize) -> Result<Self, Error> {
		if !bytes.starts_with(SIGNATURE) {
			return Err(Error::Corrupt("missing mish block-map signature"));
		}
		if binary::u32_at(bytes, VERSION_OFFSET)? != VERSION {
			return Err(Error::Unsupported("mish version"));
		}
		let count = usize::try_from(binary::u32_at(bytes, RUN_COUNT_OFFSET)?)
			.map_err(|_| Error::ResourceLimit("run records"))?;
		if count > run_limit {
			return Err(Error::ResourceLimit("run records"));
		}
		if count == 0 || Some(bytes.len()) != encoded_len(count) {
			return Err(Error::Corrupt(
				"mish record count does not match its length",
			));
		}
		if bytes[RESERVED_START..CHECKSUM_OFFSET]
			.iter()
			.any(|&byte| byte != 0)
		{
			return Err(Error::Unsupported("extended mish block-map header"));
		}
		let records = bytes[HEADER_BYTES..]
			.as_chunks::<RUN_BYTES>()
			.0
			.iter()
			.map(decode_record)
			.collect::<Result<_, _>>()?;
		Ok(Self {
			first_sector: binary::u64_at(bytes, FIRST_SECTOR_OFFSET)?,
			sector_count: binary::u64_at(bytes, SECTOR_COUNT_OFFSET)?,
			base: binary::u64_at(bytes, DATA_BASE_OFFSET)?,
			buffer_sectors: binary::u32_at(bytes, BUFFER_SECTORS_OFFSET)?,
			descriptor: binary::u32_at(bytes, DESCRIPTOR_OFFSET)?,
			checksum: Checksum::decode(bytes, CHECKSUM_OFFSET)?,
			records,
		})
	}
}

#[cfg(feature = "read")]
fn decode_record(bytes: &[u8; RUN_BYTES]) -> Result<Record, Error> {
	Ok(Record {
		kind: Kind::decode(binary::u32_at(bytes, RUN_TAG_OFFSET)?),
		comment: binary::u32_at(bytes, RUN_COMMENT_OFFSET)?,
		first_sector: binary::u64_at(bytes, RUN_FIRST_SECTOR_OFFSET)?,
		sector_count: binary::u64_at(bytes, RUN_SECTOR_COUNT_OFFSET)?,
		offset: binary::u64_at(bytes, RUN_DATA_OFFSET)?,
		stored_len: binary::u64_at(bytes, RUN_STORED_LEN_OFFSET)?,
	})
}

pub(crate) fn encoded_len(run_count: usize) -> Option<usize> {
	HEADER_BYTES.checked_add(run_count.checked_mul(RUN_BYTES)?)
}
