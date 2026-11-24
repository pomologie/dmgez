use std::mem::size_of;

use crc32fast::Hasher;

#[cfg(feature = "read")]
use crate::{Error, binary};

#[cfg(feature = "read")]
const NO_CHECKSUM_KIND: u32 = 0;
#[cfg(feature = "read")]
const NO_CHECKSUM_BITS: u32 = 0;
const CRC32_CHECKSUM_KIND: u32 = 2;
const CRC32_BITS: u32 = u32::BITS;
const BIT_COUNT_OFFSET: usize = size_of::<u32>();
const PAYLOAD_OFFSET: usize = BIT_COUNT_OFFSET + size_of::<u32>();
#[cfg(feature = "read")]
const PAYLOAD_CAPACITY_BITS: u32 = 128 * u8::BITS;

#[cfg(feature = "read")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checksum {
	/// No checksum was declared for this domain.
	Absent,
	/// Standard CRC32 over the domain defined by the UDIF profile.
	Crc32(u32),
	/// An unimplemented on-disk checksum descriptor.
	Unsupported { kind: u32, size_bits: u32 },
}

#[cfg(feature = "read")]
impl Checksum {
	pub(crate) fn decode(bytes: &[u8], descriptor_offset: usize) -> Result<Self, Error> {
		let kind = binary::u32_at(bytes, descriptor_offset)?;
		let size_bits = binary::u32_at(bytes, descriptor_offset + BIT_COUNT_OFFSET)?;
		if size_bits > PAYLOAD_CAPACITY_BITS {
			return Err(Error::Corrupt("checksum exceeds its descriptor"));
		}
		match (kind, size_bits) {
			(NO_CHECKSUM_KIND, NO_CHECKSUM_BITS) => Ok(Self::Absent),
			(CRC32_CHECKSUM_KIND, CRC32_BITS) => Ok(Self::Crc32(binary::u32_at(
				bytes,
				descriptor_offset + PAYLOAD_OFFSET,
			)?)),
			(NO_CHECKSUM_KIND | CRC32_CHECKSUM_KIND, _) => {
				Err(Error::Corrupt("inconsistent checksum width"))
			}
			_ => Ok(Self::Unsupported { kind, size_bits }),
		}
	}

	pub(crate) fn compare(self, actual: u32, domain: impl FnOnce() -> String) -> Result<(), Error> {
		match self {
			Self::Crc32(expected) if expected != actual => Err(Error::ChecksumMismatch {
				domain: domain(),
				expected,
				actual,
			}),
			Self::Unsupported { .. } => Err(Error::Unsupported("checksum algorithm")),
			Self::Crc32(_) | Self::Absent => Ok(()),
		}
	}
}

pub(crate) fn master_crc(table_crcs: impl IntoIterator<Item = u32>) -> u32 {
	let mut hash = Hasher::new();
	for crc in table_crcs {
		hash.update(&crc.to_be_bytes());
	}
	hash.finalize()
}
