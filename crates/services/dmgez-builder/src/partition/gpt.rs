use std::io::{self, Read, Seek, SeekFrom, Write};

use dmgez_udif::{SECTOR_BYTES, writer::Cancellation};
use gpt::{
	disk::LogicalBlockSize, header::Header, mbr::ProtectiveMBR, partition::Partition,
	partition_types::MACOS_HFSPLUS,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{Envelope, Error, GPT_EDGE_SECTORS, MAP_WORK_BYTES, VOLUME_PARTITION_NAME};

const MAGIC: &str = "EFI PART";
const REVISION: (u16, u16) = (1, 0);
const HEADER_BYTES: u32 = 92;
const ENTRY_BYTES: u32 = 128;
const ENTRY_COUNT: u32 = 128;
const ARRAY_SECTORS: u64 = (ENTRY_COUNT as u64 * ENTRY_BYTES as u64) / SECTOR_BYTES;
const PRIMARY_LBA: u64 = 1;
const PRIMARY_ARRAY_LBA: u64 = 2;
const VOLUME_ENTRY_INDEX: u64 = 0;
const LOGICAL_BLOCK: LogicalBlockSize = LogicalBlockSize::Lb512;
const MAP_REGION_BYTES: usize = (GPT_EDGE_SECTORS * SECTOR_BYTES) as usize;
const HASH_BUFFER_BYTES: usize = MAP_WORK_BYTES;
const DISK_UUID_DOMAIN: &[u8] = b"dmgez:gpt-disk:v1\0";
const PARTITION_UUID_DOMAIN: &[u8] = b"dmgez:gpt-partition:v1\0";

pub(super) fn generate(
	volume_bytes: u64,
	disk_bytes: u64,
	volume: impl Read,
	cancellation: &Cancellation,
) -> Result<Envelope, Error> {
	let digest = hash_volume(volume, volume_bytes, cancellation)?;
	let mut device = MapRegions::new(disk_bytes);
	let final_lba = disk_bytes / SECTOR_BYTES - 1;
	let backup_array_lba = final_lba - ARRAY_SECTORS;
	let mbr_extent = u32::try_from(final_lba).unwrap_or(u32::MAX);
	ProtectiveMBR::with_lb_size(mbr_extent)
		.overwrite_lba0(&mut device)
		.map_err(serialization_error)?;

	let partition = Partition {
		part_type_guid: MACOS_HFSPLUS,
		part_guid: guid(PARTITION_UUID_DOMAIN, &digest),
		first_lba: GPT_EDGE_SECTORS,
		last_lba: GPT_EDGE_SECTORS + volume_bytes / SECTOR_BYTES - 1,
		flags: 0,
		name: VOLUME_PARTITION_NAME.to_owned(),
	};
	for array in [PRIMARY_ARRAY_LBA, backup_array_lba] {
		partition
			.write_to_device(
				&mut device,
				VOLUME_ENTRY_INDEX,
				array,
				LOGICAL_BLOCK,
				ENTRY_BYTES,
			)
			.map_err(serialization_error)?;
	}

	let mut primary = Header {
		signature: MAGIC.to_owned(),
		revision: REVISION,
		header_size_le: HEADER_BYTES,
		crc32: 0,
		reserved: 0,
		current_lba: PRIMARY_LBA,
		backup_lba: final_lba,
		first_usable: PRIMARY_ARRAY_LBA + ARRAY_SECTORS,
		last_usable: backup_array_lba - 1,
		disk_guid: guid(DISK_UUID_DOMAIN, &digest),
		part_start: PRIMARY_ARRAY_LBA,
		num_parts: ENTRY_COUNT,
		part_size: ENTRY_BYTES,
		crc32_parts: 0,
	};
	let mut backup = Header {
		current_lba: final_lba,
		backup_lba: PRIMARY_LBA,
		part_start: backup_array_lba,
		..primary.clone()
	};
	primary
		.write_primary(&mut device, LOGICAL_BLOCK)
		.map_err(serialization_error)?;
	backup
		.write_backup(&mut device, LOGICAL_BLOCK)
		.map_err(serialization_error)?;
	Ok(device.maps)
}

fn hash_volume(
	mut volume: impl Read,
	mut remaining: u64,
	cancellation: &Cancellation,
) -> Result<[u8; 32], Error> {
	let mut hash = Sha256::new();
	let mut buffer = [0; HASH_BUFFER_BYTES];
	while remaining > 0 {
		if cancellation.is_cancelled() {
			return Err(Error::Cancelled);
		}
		let count = remaining.min(HASH_BUFFER_BYTES as u64) as usize;
		volume
			.read_exact(&mut buffer[..count])
			.map_err(Error::HashVolume)?;
		hash.update(&buffer[..count]);
		remaining -= count as u64;
	}
	if cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	if volume.read(&mut buffer[..1]).map_err(Error::HashVolume)? != 0 {
		return Err(Error::Geometry);
	}
	Ok(hash.finalize().into())
}

fn guid(domain: &[u8], digest: &[u8; 32]) -> Uuid {
	let mut hash = Sha256::new();
	hash.update(domain);
	hash.update(digest);
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&hash.finalize()[..16]);
	uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn serialization_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
	Error::Generate(Box::new(error))
}

/// An address space whose admitted writable regions are just the two maps.
#[derive(Debug)]
struct MapRegions {
	maps: Envelope,
	position: u64,
	length: u64,
}

impl MapRegions {
	fn new(length: u64) -> Self {
		Self {
			maps: Envelope {
				prefix: vec![0; MAP_REGION_BYTES],
				suffix: vec![0; MAP_REGION_BYTES],
			},
			position: 0,
			length,
		}
	}

	fn region(&mut self) -> io::Result<&mut [u8]> {
		if self.position < MAP_REGION_BYTES as u64 {
			return Ok(&mut self.maps.prefix[self.position as usize..]);
		}
		let tail_start = self.length - MAP_REGION_BYTES as u64;
		if let Some(relative) = self.position.checked_sub(tail_start) {
			return Ok(&mut self.maps.suffix[relative as usize..]);
		}
		Err(io::ErrorKind::InvalidData.into())
	}
}

impl Read for MapRegions {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		let mut region: &[u8] = self.region()?;
		let count = region.read(output)?;
		self.position += count as u64;
		Ok(count)
	}
}

impl Write for MapRegions {
	fn write(&mut self, input: &[u8]) -> io::Result<usize> {
		let count = self.region()?.write(input)?;
		self.position += count as u64;
		Ok(count)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl Seek for MapRegions {
	fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
		let target = match position {
			SeekFrom::Start(offset) => Some(offset),
			SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
			SeekFrom::End(delta) => self.length.checked_add_signed(delta),
		};
		self.position = target
			.filter(|&target| target <= self.length)
			.ok_or(io::ErrorKind::InvalidInput)?;
		Ok(self.position)
	}
}
