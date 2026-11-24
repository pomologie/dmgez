use std::{num::NonZeroU32, ops::Range};

use dmgez_udif::SECTOR_BYTES;

use crate::{
	Error, Filesystem, Layout, Limits, LogicalDisk, Partition, PartitionId, Scheme, apm, gpt, mbr,
};

pub(super) const SECTOR: usize = SECTOR_BYTES as usize;
const HFS_HEADER_OFFSET: u64 = 1024;
const HFS_HEADER_BYTES: usize = 512;
const HFS_PLUS_SIGNATURE: u16 = 0x482B;
const HFS_X_SIGNATURE: u16 = 0x4858;
const HFS_PLUS_VERSION: u16 = 4;
const HFS_X_VERSION: u16 = 5;
const HFS_BLOCK_BYTES: usize = 40;
const HFS_BLOCK_COUNT: usize = 44;

pub(super) fn discover(source: &mut impl LogicalDisk, limits: Limits) -> Result<Layout, Error> {
	let mut probe = Probe::new(source, limits);
	let zero = probe.optional::<SECTOR>(0)?;
	let one = probe.optional::<SECTOR>(SECTOR_BYTES)?;
	let gpt_size = gpt::detect(&mut probe, one.as_ref())?;
	let mbr = zero.as_ref().map(mbr::detect).transpose()?.flatten();
	let apple = apm::detect(zero.as_ref(), one.as_ref());
	let raw = raw_volume(&mut probe)?;
	let map = match (apple, gpt_size, mbr) {
		(true, Some(_), _) | (true, None, Some(_)) => {
			return Err(conflict("multiple partition map signatures"));
		}
		(true, None, None) => Some(apm::read(&mut probe, zero.as_ref(), one.as_ref())?),
		(false, Some(block_bytes), Some(mbr)) => {
			mbr::validate_protective(&mbr, probe.len(), block_bytes)?;
			Some(gpt::read(&mut probe, block_bytes)?)
		}
		(false, Some(_), None) => return Err(corrupt(Scheme::Gpt, "missing protective MBR")),
		(false, None, Some(mbr)) if mbr::has_protective(&mbr) => {
			return Err(corrupt(
				Scheme::Gpt,
				"protective MBR without a supported GPT header",
			));
		}
		(false, None, Some(mbr)) => Some(mbr::read(&mut probe, mbr)?),
		(false, None, None) => None,
	};
	match (map, raw) {
		(Some(_), Some(_)) => Err(conflict(
			"partition map and whole-disk filesystem both present",
		)),
		(Some(layout), None) | (None, Some(layout)) => Ok(layout),
		(None, None) => Ok(Layout::Unknown),
	}
}

pub(super) struct Probe<'source, S> {
	source: &'source mut S,
	pub(super) limits: Limits,
	remaining: u64,
}

impl<'source, S: LogicalDisk> Probe<'source, S> {
	fn new(source: &'source mut S, limits: Limits) -> Self {
		Self {
			source,
			limits,
			remaining: limits.max_probe_bytes,
		}
	}

	pub(super) fn len(&self) -> u64 {
		self.source.len()
	}

	pub(super) fn optional<const N: usize>(
		&mut self,
		offset: u64,
	) -> Result<Option<[u8; N]>, Error> {
		if offset
			.checked_add(N as u64)
			.is_none_or(|end| end > self.len())
		{
			return Ok(None);
		}
		self.fixed(offset).map(Some)
	}

	pub(super) fn fixed<const N: usize>(&mut self, offset: u64) -> Result<[u8; N], Error> {
		let mut bytes = [0; N];
		self.read(offset, &mut bytes)?;
		Ok(bytes)
	}

	pub(super) fn owned(&mut self, offset: u64, len: u64) -> Result<Vec<u8>, Error> {
		self.admit(offset, len)?;
		let len = usize::try_from(len).map_err(|_| Error::Limit("partition table allocation"))?;
		let mut bytes = Vec::new();
		bytes
			.try_reserve_exact(len)
			.map_err(|_| Error::Limit("partition table allocation"))?;
		bytes.resize(len, 0);
		self.read(offset, &mut bytes)?;
		Ok(bytes)
	}

	fn admit(&self, offset: u64, len: u64) -> Result<(), Error> {
		if len > self.remaining {
			return Err(Error::Limit("partition probe bytes"));
		}
		if offset.checked_add(len).is_none_or(|end| end > self.len()) {
			return Err(Error::OutOfRange {
				offset,
				len,
				available: self.len(),
			});
		}
		Ok(())
	}

	fn read(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
		let len = bytes.len() as u64;
		self.admit(offset, len)?;
		self.remaining -= len;
		Ok(self.source.read_exact_at(offset, bytes)?)
	}
}

fn raw_volume(probe: &mut Probe<'_, impl LogicalDisk>) -> Result<Option<Layout>, Error> {
	let Some(signature) = probe.optional::<4>(HFS_HEADER_OFFSET)? else {
		return Ok(None);
	};
	let [s0, s1, v0, v1] = signature;
	let filesystem = match (u16::from_be_bytes([s0, s1]), u16::from_be_bytes([v0, v1])) {
		(HFS_PLUS_SIGNATURE, HFS_PLUS_VERSION) => Filesystem::HfsPlus,
		(HFS_X_SIGNATURE, HFS_X_VERSION) => Filesystem::HfsX,
		_ => return Ok(None),
	};
	let header = probe
		.optional::<HFS_HEADER_BYTES>(HFS_HEADER_OFFSET)?
		.ok_or(Error::VolumeGeometry)?;
	let block_bytes = be32(&header, HFS_BLOCK_BYTES);
	let block_count = be32(&header, HFS_BLOCK_COUNT);
	let extent = u64::from(block_bytes) * u64::from(block_count);
	if block_bytes < SECTOR as u32
		|| !block_bytes.is_power_of_two()
		|| extent < HFS_HEADER_OFFSET + HFS_HEADER_BYTES as u64
		|| extent > probe.len()
	{
		return Err(Error::VolumeGeometry);
	}
	Ok(Some(Layout::Partitionless {
		filesystem,
		range: 0..extent,
	}))
}

pub(super) fn partition_range(
	start: u64,
	count: u64,
	block_bytes: u64,
	disk_bytes: u64,
	scheme: Scheme,
) -> Result<Range<u64>, Error> {
	(start
		.checked_add(count)
		.and_then(|end| Some(start.checked_mul(block_bytes)?..end.checked_mul(block_bytes)?)))
	.filter(|range| count != 0 && range.end <= disk_bytes)
	.ok_or(corrupt(
		scheme,
		"partition extent is zero, overflows, or leaves the logical disk",
	))
}

pub(super) fn check_overlap<'a>(
	partitions: impl Iterator<Item = &'a Partition>,
) -> Result<(), Error> {
	let mut ranges: Vec<_> = partitions.map(|partition| &partition.range).collect();
	ranges.sort_unstable_by_key(|range| range.start);
	if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
		return Err(conflict("overlapping partition extents"));
	}
	Ok(())
}

pub(super) fn id(scheme: Scheme, slot: u32) -> Result<PartitionId, Error> {
	let slot = NonZeroU32::new(slot).ok_or(corrupt(scheme, "partition slot is zero"))?;
	Ok(PartitionId { scheme, slot })
}

pub(super) const fn corrupt(scheme: Scheme, reason: &'static str) -> Error {
	Error::Corrupt { scheme, reason }
}
pub(super) const fn unsupported(scheme: Scheme, reason: &'static str) -> Error {
	Error::Unsupported { scheme, reason }
}
pub(super) const fn conflict(reason: &'static str) -> Error {
	Error::Conflict { reason }
}

pub(super) fn be32(bytes: &[u8], offset: usize) -> u32 {
	u32::from_be_bytes(
		bytes[offset..][..4]
			.try_into()
			.expect("fixed four-byte field"),
	)
}
