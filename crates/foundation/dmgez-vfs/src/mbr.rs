use std::collections::HashSet;

use dmgez_udif::SECTOR_BYTES;
use part_tables::mbr::{Mbr, MbrPartitionEntry};

use crate::{
	Error, Layout, LogicalDisk, Partition, PartitionKind, PartitionRole, Scheme,
	discovery::{
		Probe, SECTOR, check_overlap, conflict, corrupt, id, partition_range, unsupported,
	},
};

const SCHEME: Scheme = Scheme::Mbr;
const MBR_MAGIC: [u8; 2] = [0x55, 0xAA];
const TABLE_OFFSET: usize = 446;
const ENTRY_BYTES: usize = 16;
const PRIMARY_SLOTS: u32 = 4;
const INACTIVE: u8 = 0x00;
const BOOTABLE: u8 = 0x80;
const GPT_PROTECTIVE_TYPE: u8 = 0xEE;
const FIRST_GPT_LBA: u32 = 1;
const MAX_LOGICAL_ENTRIES: u32 = 1024;

pub(super) fn detect(sector: &[u8; SECTOR]) -> Result<Option<Mbr>, Error> {
	if !sector.ends_with(&MBR_MAGIC) {
		return Ok(None);
	}
	let table = Mbr::parse(sector).map_err(|_| corrupt(SCHEME, "invalid master boot record"))?;
	if table.entries.iter().all(MbrPartitionEntry::is_empty) {
		return Ok(None);
	}
	validate_status(sector)?;
	Ok(Some(table))
}

pub(super) fn has_protective(table: &Mbr) -> bool {
	table
		.entries
		.iter()
		.any(|entry| entry.partition_type == GPT_PROTECTIVE_TYPE)
}

pub(super) fn validate_protective(table: &Mbr, bytes: u64, block_bytes: u64) -> Result<(), Error> {
	let active = table
		.entries
		.iter()
		.filter(|entry| !entry.is_empty())
		.collect::<Vec<_>>();
	if active.len() != 1 || active[0].partition_type != GPT_PROTECTIVE_TYPE {
		return Err(conflict("hybrid GPT and MBR partition records"));
	}
	let protective = active[0];
	let sectors = u32::try_from((bytes / block_bytes).saturating_sub(1)).unwrap_or(u32::MAX);
	if protective.start_lba != FIRST_GPT_LBA
		|| protective.total_sectors != sectors
		|| protective.bootable
	{
		return Err(corrupt(
			Scheme::Gpt,
			"protective MBR does not cover GPT disk",
		));
	}
	Ok(())
}

pub(super) fn read(probe: &mut Probe<'_, impl LogicalDisk>, table: Mbr) -> Result<Layout, Error> {
	if !probe.len().is_multiple_of(SECTOR_BYTES) {
		return Err(corrupt(
			SCHEME,
			"disk size is not a whole number of MBR sectors",
		));
	}
	let mut partitions = Vec::new();
	let mut extended = None;
	for (index, entry) in table.entries.into_iter().enumerate() {
		if entry.is_empty() {
			continue;
		}
		let start = u64::from(entry.start_lba);
		if start == 0 {
			return Err(corrupt(
				SCHEME,
				"primary entry covers the master boot record",
			));
		}
		let partition = make_partition(index as u32 + 1, start, &entry, probe.len())?;
		if entry.is_extended() && extended.replace(partition.range.clone()).is_some() {
			return Err(unsupported(SCHEME, "multiple extended containers"));
		}
		partitions.push(partition);
	}
	if partitions.len() > probe.limits.max_entries as usize {
		return Err(Error::Limit("MBR primary entry count"));
	}
	check_overlap(partitions.iter())?;
	if let Some(range) = extended {
		partitions.extend(read_chain(probe, range.start / SECTOR_BYTES, range.end)?);
	}
	Ok(Layout::Partitioned {
		scheme: SCHEME,
		block_bytes: SECTOR_BYTES,
		partitions,
	})
}

fn read_chain(
	probe: &mut Probe<'_, impl LogicalDisk>,
	base: u64,
	end: u64,
) -> Result<Vec<Partition>, Error> {
	let mut output = Vec::new();
	let mut visited = HashSet::new();
	let mut current = base;
	loop {
		if !visited.insert(current) {
			return Err(corrupt(SCHEME, "extended boot record cycle"));
		}
		if visited.len() > MAX_LOGICAL_ENTRIES.min(probe.limits.max_entries) as usize {
			return Err(Error::Limit("extended MBR chain length"));
		}
		let extent = partition_range(current, 1, SECTOR_BYTES, end, SCHEME)?;
		let bytes = probe.fixed::<SECTOR>(extent.start)?;
		let table = detect(&bytes)?.ok_or(corrupt(SCHEME, "missing extended boot record"))?;
		let [data, link, third, fourth] = table.entries;
		if !third.is_empty()
			|| !fourth.is_empty()
			|| data.is_empty()
			|| data.is_extended()
			|| data.start_lba == 0
			|| data.partition_type == GPT_PROTECTIVE_TYPE
		{
			return Err(corrupt(SCHEME, "invalid extended boot record entries"));
		}
		output.push(make_partition(
			PRIMARY_SLOTS + output.len() as u32 + 1,
			current + u64::from(data.start_lba),
			&data,
			end,
		)?);
		if link.is_empty() {
			break;
		}
		if !link.is_extended() {
			return Err(corrupt(SCHEME, "extended boot record link is not extended"));
		}
		current = base + u64::from(link.start_lba);
		partition_range(
			current,
			u64::from(link.total_sectors),
			SECTOR_BYTES,
			end,
			SCHEME,
		)?;
	}
	check_overlap(output.iter())?;
	let mut extents: Vec<_> = output.iter().map(|partition| &partition.range).collect();
	extents.sort_unstable_by_key(|range| range.start);
	for lba in visited {
		let offset = lba * SECTOR_BYTES;
		let index = extents.partition_point(|range| range.end <= offset);
		if extents
			.get(index)
			.is_some_and(|range| range.contains(&offset))
		{
			return Err(conflict("logical partition covers an extended boot record"));
		}
	}
	Ok(output)
}

fn make_partition(
	slot: u32,
	start: u64,
	entry: &MbrPartitionEntry,
	bound: u64,
) -> Result<Partition, Error> {
	if entry.partition_type == 0 {
		return Err(corrupt(SCHEME, "untyped MBR entry claims space"));
	}
	let range = partition_range(
		start,
		u64::from(entry.total_sectors),
		SECTOR_BYTES,
		bound,
		SCHEME,
	)?;
	let role = if entry.is_extended() {
		PartitionRole::ExtendedContainer
	} else {
		PartitionRole::Data
	};
	Ok(Partition {
		id: id(SCHEME, slot)?,
		role,
		data_range: (role == PartitionRole::Data).then(|| range.clone()),
		range,
		kind: PartitionKind::Mbr {
			type_code: entry.partition_type,
			bootable: entry.bootable,
		},
	})
}

fn validate_status(sector: &[u8; SECTOR]) -> Result<(), Error> {
	for slot in 0..PRIMARY_SLOTS as usize {
		if !matches!(
			sector[TABLE_OFFSET + slot * ENTRY_BYTES],
			INACTIVE | BOOTABLE
		) {
			return Err(corrupt(SCHEME, "invalid boot status"));
		}
	}
	Ok(())
}
