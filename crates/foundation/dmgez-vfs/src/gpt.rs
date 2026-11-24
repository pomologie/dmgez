use std::{collections::HashSet, io::Cursor};

use gpt::{
	disk::LogicalBlockSize,
	header::{Header, read_header_from_arbitrary_device},
	partition::file_read_partitions,
};

use crate::{
	Error, Layout, LogicalDisk, Partition, PartitionKind, PartitionRole, Scheme,
	discovery::{
		Probe, SECTOR, check_overlap, conflict, corrupt, id, partition_range, unsupported,
	},
};

const SCHEME: Scheme = Scheme::Gpt;
const MAGIC: [u8; 8] = *b"EFI PART";
const PRIMARY_LBA: u64 = 1;
const GPT_V1: (u16, u16) = (1, 0);
const HEADER_BYTES: u32 = 92;
const ENTRY_BYTES: u32 = 128;
const MAX_ENTRIES: u32 = 16_384;
const MIN_ARRAY_BYTES: u64 = 16 << 10;
const FOUR_KIB: u64 = 4096;

pub(super) fn detect(
	probe: &mut Probe<'_, impl LogicalDisk>,
	one: Option<&[u8; SECTOR]>,
) -> Result<Option<u64>, Error> {
	let normal = one.is_some_and(|sector| sector.starts_with(&MAGIC));
	let large = probe
		.optional::<{ MAGIC.len() }>(FOUR_KIB)?
		.is_some_and(|bytes| bytes == MAGIC);
	match (normal, large) {
		(true, true) => Err(conflict("multiple GPT logical block sizes")),
		(true, false) => Ok(Some(SECTOR as u64)),
		(false, true) => Ok(Some(FOUR_KIB)),
		(false, false) => Ok(None),
	}
}

pub(super) fn read(
	probe: &mut Probe<'_, impl LogicalDisk>,
	block_bytes: u64,
) -> Result<Layout, Error> {
	if !probe.len().is_multiple_of(block_bytes) || probe.len() / block_bytes <= PRIMARY_LBA + 1 {
		return Err(corrupt(
			SCHEME,
			"disk size is inconsistent with GPT logical blocks",
		));
	}
	let final_lba = probe.len() / block_bytes - 1;
	let primary = read_header(probe, PRIMARY_LBA, block_bytes)?;
	let backup = read_header(probe, final_lba, block_bytes)?;
	validate_pair(&primary, &backup, final_lba)?;
	if primary.num_parts > MAX_ENTRIES.min(probe.limits.max_entries) {
		return Err(Error::Limit("GPT entry array count"));
	}
	let array_bytes = u64::from(primary.num_parts) * u64::from(ENTRY_BYTES);
	let map_blocks = array_bytes.div_ceil(block_bytes);
	if array_bytes < MIN_ARRAY_BYTES
		|| primary.part_start <= PRIMARY_LBA
		|| primary
			.part_start
			.checked_add(map_blocks)
			.is_none_or(|end| end > primary.first_usable)
		|| backup.part_start <= primary.last_usable
		|| backup
			.part_start
			.checked_add(map_blocks)
			.is_none_or(|end| end > final_lba)
	{
		return Err(corrupt(SCHEME, "entry arrays leave their metadata regions"));
	}
	let first = probe.owned(primary.part_start * block_bytes, array_bytes)?;
	let second = probe.owned(backup.part_start * block_bytes, array_bytes)?;
	if first != second {
		return Err(corrupt(SCHEME, "primary and backup entry arrays differ"));
	}
	let mut decoder_header = primary.clone();
	decoder_header.part_start = 0;
	let entries = file_read_partitions(
		&mut Cursor::new(first),
		&decoder_header,
		LogicalBlockSize::Lb512,
	)
	.map_err(|_| corrupt(SCHEME, "invalid partition entry array or CRC"))?;
	let mut partitions = Vec::with_capacity(entries.len());
	let mut guids = HashSet::with_capacity(entries.len());
	for (slot, entry) in entries {
		if entry.first_lba < primary.first_usable
			|| entry.last_lba > primary.last_usable
			|| entry.first_lba > entry.last_lba
			|| entry.part_guid.is_nil()
			|| entry.part_type_guid.guid.is_nil()
		{
			return Err(corrupt(
				SCHEME,
				"entry GUID or usable LBA extent is invalid",
			));
		}
		if !guids.insert(entry.part_guid) {
			return Err(conflict("duplicate GPT partition GUID"));
		}
		let range = partition_range(
			entry.first_lba,
			entry.last_lba - entry.first_lba + 1,
			block_bytes,
			probe.len(),
			SCHEME,
		)?;
		partitions.push(Partition {
			id: id(SCHEME, slot)?,
			role: PartitionRole::Data,
			data_range: Some(range.clone()),
			range,
			kind: PartitionKind::Gpt {
				name: entry.name,
				type_guid: *entry.part_type_guid.guid.as_bytes(),
				unique_guid: *entry.part_guid.as_bytes(),
				attributes: entry.flags,
			},
		});
	}
	check_overlap(partitions.iter())?;
	Ok(Layout::Partitioned {
		scheme: SCHEME,
		block_bytes,
		partitions,
	})
}

fn read_header(
	probe: &mut Probe<'_, impl LogicalDisk>,
	lba: u64,
	block_bytes: u64,
) -> Result<Header, Error> {
	let offset = lba
		.checked_mul(block_bytes)
		.ok_or(corrupt(SCHEME, "header byte offset overflow"))?;
	// The library's public header decoder seeks to one sector. Give it only
	// this already bounded header and retain the original LBA for validation.
	let bytes = probe.fixed::<{ HEADER_BYTES as usize }>(offset)?;
	let mut input = [0; SECTOR + HEADER_BYTES as usize];
	input[SECTOR..].copy_from_slice(&bytes);
	let header =
		read_header_from_arbitrary_device(&mut Cursor::new(input), LogicalBlockSize::Lb512)
			.map_err(|_| corrupt(SCHEME, "invalid header or CRC"))?;
	if header.revision != GPT_V1
		|| header.header_size_le != HEADER_BYTES
		|| header.part_size != ENTRY_BYTES
	{
		return Err(unsupported(
			SCHEME,
			"header revision, header size, or entry size",
		));
	}
	if header.reserved != 0 || header.current_lba != lba || header.disk_guid.is_nil() {
		return Err(corrupt(SCHEME, "header identity, reserved field, or LBA"));
	}
	Ok(header)
}

fn validate_pair(primary: &Header, backup: &Header, final_lba: u64) -> Result<(), Error> {
	if primary.backup_lba != final_lba
		|| backup.backup_lba != PRIMARY_LBA
		|| primary.disk_guid != backup.disk_guid
		|| (primary.first_usable, primary.last_usable) != (backup.first_usable, backup.last_usable)
		|| primary.num_parts != backup.num_parts
		|| primary.crc32_parts != backup.crc32_parts
	{
		return Err(corrupt(SCHEME, "primary and backup headers disagree"));
	}
	if primary.first_usable > primary.last_usable || primary.last_usable >= final_lba {
		return Err(corrupt(SCHEME, "invalid usable LBA interval"));
	}
	Ok(())
}
