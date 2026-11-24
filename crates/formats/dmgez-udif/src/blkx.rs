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
const RUN_TAG_LZFSE: u32 = 0x8000_0007;
const RUN_TAG_LZMA2: u32 = 0x8000_0008;
#[cfg(feature = "read")]
const RUN_TAG_COMMENT: u32 = 0x7fff_fffe;
const RUN_TAG_END: u32 = 0xffff_ffff;

pub(crate) const NO_RUN_COMMENT: u32 = 0;

#[cfg(feature = "write")]
const WHOLE_DISK_DESCRIPTOR: u32 = 0xffff_fffe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
	Raw,
	Zero,
	#[cfg(feature = "read")]
	Ignore,
	Zlib,
	Lzfse,
	Lzma2,
	#[cfg(feature = "read")]
	Comment,
	End,
	#[cfg(feature = "read")]
	Unsupported(u32),
}

impl Kind {
	const COMPRESSED: [Self; 3] = [Self::Zlib, Self::Lzfse, Self::Lzma2];

	pub(crate) fn is_compressed(self) -> bool {
		Self::COMPRESSED.contains(&self)
	}

	#[cfg(feature = "read")]
	fn decode(code: u32) -> Self {
		match code {
			RUN_TAG_ZERO => Self::Zero,
			RUN_TAG_RAW => Self::Raw,
			RUN_TAG_IGNORE => Self::Ignore,
			RUN_TAG_ZLIB => Self::Zlib,
			RUN_TAG_LZFSE => Self::Lzfse,
			RUN_TAG_LZMA2 => Self::Lzma2,
			RUN_TAG_COMMENT => Self::Comment,
			RUN_TAG_END => Self::End,
			other => Self::Unsupported(other),
		}
	}

	#[cfg(feature = "write")]
	fn encode(self) -> u32 {
		match self {
			Self::Zero => RUN_TAG_ZERO,
			Self::Raw => RUN_TAG_RAW,
			#[cfg(feature = "read")]
			Self::Ignore => RUN_TAG_IGNORE,
			Self::Zlib => RUN_TAG_ZLIB,
			Self::Lzfse => RUN_TAG_LZFSE,
			Self::Lzma2 => RUN_TAG_LZMA2,
			#[cfg(feature = "read")]
			Self::Comment => RUN_TAG_COMMENT,
			Self::End => RUN_TAG_END,
			#[cfg(feature = "read")]
			Self::Unsupported(code) => code,
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

#[cfg(feature = "write")]
pub(crate) fn encode(
	sectors: u64,
	buffer_sectors: u32,
	crc: u32,
	records: &[Record],
) -> Result<Vec<u8>, Error> {
	let count = u32::try_from(records.len()).map_err(|_| Error::ResourceLimit("run records"))?;
	let len = encoded_len(records.len()).ok_or(Error::ResourceLimit("run records"))?;
	let mut bytes = vec![0; len];
	bytes[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
	binary::put_u32(&mut bytes, VERSION_OFFSET, VERSION);
	binary::put_u64(&mut bytes, SECTOR_COUNT_OFFSET, sectors);
	binary::put_u32(&mut bytes, BUFFER_SECTORS_OFFSET, buffer_sectors);
	binary::put_u32(&mut bytes, DESCRIPTOR_OFFSET, WHOLE_DISK_DESCRIPTOR);
	crate::checksum::put_crc32(&mut bytes, CHECKSUM_OFFSET, crc);
	binary::put_u32(&mut bytes, RUN_COUNT_OFFSET, count);
	for (record, slot) in records
		.iter()
		.zip(bytes[HEADER_BYTES..].as_chunks_mut::<RUN_BYTES>().0)
	{
		binary::put_u32(slot, RUN_TAG_OFFSET, record.kind.encode());
		binary::put_u32(slot, RUN_COMMENT_OFFSET, record.comment);
		binary::put_u64(slot, RUN_FIRST_SECTOR_OFFSET, record.first_sector);
		binary::put_u64(slot, RUN_SECTOR_COUNT_OFFSET, record.sector_count);
		binary::put_u64(slot, RUN_DATA_OFFSET, record.offset);
		binary::put_u64(slot, RUN_STORED_LEN_OFFSET, record.stored_len);
	}
	Ok(bytes)
}
