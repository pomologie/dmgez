use std::io::Cursor;

use dmgez_udif::SECTOR_BYTES;
use part_tables::apm::Apm;

use crate::{
	Error, Layout, LogicalDisk, Partition, PartitionKind, PartitionRole, Scheme,
	discovery::{Probe, SECTOR, be32, check_overlap, corrupt, id, partition_range, unsupported},
};

const SCHEME: Scheme = Scheme::Apple;
const DRIVER_MAP_MAGIC: [u8; 2] = *b"ER";
const PARTITION_MAP_MAGIC: [u8; 2] = *b"PM";
const DRIVER_BLOCK_BYTES_OFFSET: usize = 2;
const DRIVER_ENTRY_COUNT_OFFSET: usize = 16;
const MAX_DRIVER_ENTRIES: u16 = 61;
const MAP_COUNT_OFFSET: usize = 4;
const MAX_MAP_ENTRIES: u32 = 128;
const MAP_PARTITION_LBA: u64 = 1;
const MAP_PARTITION_TYPE: &str = "Apple_partition_map";
const FREE_PARTITION_TYPE: &str = "Apple_Free";

pub(super) fn detect(zero: Option<&[u8; SECTOR]>, one: Option<&[u8; SECTOR]>) -> bool {
	zero.is_some_and(|bytes| bytes.starts_with(&DRIVER_MAP_MAGIC))
		|| one.is_some_and(|bytes| bytes.starts_with(&PARTITION_MAP_MAGIC))
}

pub(super) fn read(
	probe: &mut Probe<'_, impl LogicalDisk>,
	zero: Option<&[u8; SECTOR]>,
	one: Option<&[u8; SECTOR]>,
) -> Result<Layout, Error> {
	let driver = zero
		.filter(|bytes| bytes.starts_with(&DRIVER_MAP_MAGIC))
		.ok_or(unsupported(SCHEME, "map without a driver descriptor"))?;
	let block_bytes = field16(driver, DRIVER_BLOCK_BYTES_OFFSET);
	if u64::from(block_bytes) != SECTOR_BYTES {
		return Err(unsupported(SCHEME, "driver descriptor block size"));
	}
	if field16(driver, DRIVER_ENTRY_COUNT_OFFSET) > MAX_DRIVER_ENTRIES {
		return Err(corrupt(SCHEME, "driver count exceeds its descriptor"));
	}
	let first = one
		.filter(|bytes| bytes.starts_with(&PARTITION_MAP_MAGIC))
		.ok_or(corrupt(SCHEME, "missing first map entry"))?;
	let count = be32(first, MAP_COUNT_OFFSET);
	if count == 0 {
		return Err(corrupt(SCHEME, "zero map entries"));
	}
	if count > MAX_MAP_ENTRIES.min(probe.limits.max_entries) {
		return Err(Error::Limit("APM map records"));
	}
	let map_bytes = (u64::from(count) + 1) * SECTOR_BYTES;
	if map_bytes > probe.len() {
		return Err(corrupt(SCHEME, "map records leave the logical disk"));
	}
	let metadata = probe.owned(0, map_bytes)?;
	let map = Apm::parse(&mut Cursor::new(metadata))
		.map_err(|_| corrupt(SCHEME, "could not decode partition entries"))?;
	let disk_bytes = u64::from(map.ddr.block_count) * SECTOR_BYTES;
	if disk_bytes < map_bytes || disk_bytes > probe.len() {
		return Err(corrupt(
			SCHEME,
			"driver descriptor disk size exceeds logical bounds",
		));
	}
	let mut partitions = Vec::with_capacity(map.entries.len());
	let mut map_partition_count = 0;
	for (index, entry) in map.entries.into_iter().enumerate() {
		if entry.map_entries != count {
			return Err(corrupt(SCHEME, "inconsistent entry count"));
		}
		let start = u64::from(entry.start_block);
		if start < MAP_PARTITION_LBA {
			return Err(corrupt(SCHEME, "partition covers the driver descriptor"));
		}
		let range = partition_range(
			start,
			u64::from(entry.block_count),
			SECTOR_BYTES,
			disk_bytes,
			SCHEME,
		)?;
		let role = if entry.partition_type == MAP_PARTITION_TYPE {
			map_partition_count += 1;
			if start != MAP_PARTITION_LBA || range.end < map_bytes {
				return Err(corrupt(SCHEME, "map partition does not cover all entries"));
			}
			PartitionRole::Metadata
		} else if entry.partition_type == FREE_PARTITION_TYPE {
			PartitionRole::Free
		} else if entry.is_data_partition() {
			PartitionRole::Data
		} else {
			PartitionRole::Metadata
		};
		if u64::from(entry.data_start) + u64::from(entry.data_count) > u64::from(entry.block_count)
		{
			return Err(corrupt(SCHEME, "data region leaves its partition"));
		}
		let data_range = (role == PartitionRole::Data && entry.data_count != 0)
			.then(|| {
				partition_range(
					start + u64::from(entry.data_start),
					u64::from(entry.data_count),
					SECTOR_BYTES,
					range.end,
					SCHEME,
				)
			})
			.transpose()?;
		partitions.push(Partition {
			id: id(SCHEME, index as u32 + 1)?,
			role,
			range,
			data_range,
			kind: PartitionKind::Apple {
				name: entry.name,
				type_code: entry.partition_type,
				status: entry.status,
			},
		});
	}
	if map_partition_count != 1 {
		return Err(corrupt(SCHEME, "expected one map partition"));
	}
	check_overlap(partitions.iter())?;
	Ok(Layout::Partitioned {
		scheme: SCHEME,
		block_bytes: SECTOR_BYTES,
		partitions,
	})
}

fn field16(bytes: &[u8], offset: usize) -> u16 {
	u16::from_be_bytes(
		bytes[offset..][..2]
			.try_into()
			.expect("fixed two-byte field"),
	)
}
