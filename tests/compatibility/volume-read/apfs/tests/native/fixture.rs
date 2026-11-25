use std::{
	collections::BTreeMap,
	io::{self, Read},
	num::NonZeroU32,
};

use apfs_container::{Container, OpenOptions};
use apfs_core::{ByteOffset, ReadAt};
use dmgez_apfs_volume_read_compatibility::{SourceError, WindowSource};
use dmgez_udif::{
	reader::{Image, Limits as ImageLimits},
	writer::{Profile, WriteOptions, write_image},
};
use dmgez_vfs::{Disk, Limits as DiskLimits, PartitionId, Scheme, VolumeId};
use thiserror::Error;

pub const NATIVE_BLOCK_BYTES: u64 = 4096;
const NATIVE_VOLUME_BYTES: u64 = 64 << 20;
const PACKED_ADDRESS_BYTES: usize = size_of::<u64>();
const PACKED_RECORD_BYTES: usize = PACKED_ADDRESS_BYTES + NATIVE_BLOCK_BYTES as usize;
const SECTOR_BYTES: usize = 512;
const APFS_BEGIN_SECTOR: u32 = 2048;
const APFS_BEGIN_BYTES: u64 = APFS_BEGIN_SECTOR as u64 * SECTOR_BYTES as u64;
const DISK_BYTES: u64 = APFS_BEGIN_BYTES + NATIVE_VOLUME_BYTES;
const MBR_TABLE: usize = 446;
const MBR_TYPE_OFFSET: usize = MBR_TABLE + 4;
const MBR_START_LBA: usize = MBR_TABLE + 8;
const MBR_LENGTH_LBA: usize = MBR_TABLE + 12;
const MBR_SIGNATURE_OFFSET: usize = 510;
const MBR_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const NON_DOS_DATA_PARTITION: u8 = 0xda;

pub const VOLUME: VolumeId = VolumeId::Partition(PartitionId {
	scheme: Scheme::Mbr,
	slot: NonZeroU32::MIN,
});

pub struct NativeBlocks(BTreeMap<u64, &'static [u8]>);

impl NativeBlocks {
	pub fn from_packed(packed: &'static [u8]) -> Self {
		let (records, trailing) = packed.as_chunks::<PACKED_RECORD_BYTES>();
		assert!(trailing.is_empty());
		let mut blocks = BTreeMap::new();
		for record in records {
			let (address, data) = record.split_at(PACKED_ADDRESS_BYTES);
			let block = u64::from_le_bytes(address.try_into().unwrap());
			assert!(block < NATIVE_VOLUME_BYTES / NATIVE_BLOCK_BYTES);
			assert!(blocks.insert(block, data).is_none());
		}
		Self(blocks)
	}

	pub fn wrap_udif(&self) -> Vec<u8> {
		let source = LogicalImage {
			native: self,
			header: mbr(),
			cursor: 0,
		};
		let mut encoded = Vec::new();
		let options = WriteOptions {
			profile: Profile::Udzo,
			..WriteOptions::default()
		};
		write_image(source, DISK_BYTES, &mut encoded, &options).unwrap();
		encoded
	}
}

pub fn disk(encoded: &[u8]) -> Disk<Image<&[u8]>> {
	let image = Image::open(encoded, ImageLimits::default()).unwrap();
	let disk = Disk::discover(image, DiskLimits::default()).unwrap();
	assert_eq!(disk.select_volume(Some(VOLUME)).unwrap(), VOLUME);
	disk
}

pub fn open_container<'source, 'image>(
	disk: &'source mut Disk<Image<&'image [u8]>>,
	native: &'source NativeBlocks,
) -> Container<FixtureSource<'source, 'image>> {
	let window = disk.data_window(VOLUME).unwrap();
	assert_eq!(window.len(), NATIVE_VOLUME_BYTES);
	assert_eq!(window.logical_offset(), APFS_BEGIN_BYTES);
	let source = FixtureSource {
		source: WindowSource::new(window),
		native,
	};
	Container::open(source, OpenOptions::default()).unwrap()
}

/// A fixture can reconstruct a native block only when the provenance kept it.
pub struct FixtureSource<'source, 'image> {
	source: WindowSource<'source, Image<&'image [u8]>>,
	native: &'source NativeBlocks,
}

#[derive(Debug, Error)]
pub enum FixtureError {
	#[error("read exceeds the APFS fixture partition")]
	Bounds,
	#[error("APFS fixture omitted native block {0}")]
	MissingBlock(u64),
	#[error("APFS positional source failed")]
	Bridge(#[from] SourceError),
}

impl ReadAt for FixtureSource<'_, '_> {
	type Error = FixtureError;

	fn len(&self) -> u64 {
		self.source.len()
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		let end = offset
			.0
			.checked_add(output.len() as u64)
			.filter(|&end| end <= self.len())
			.ok_or(FixtureError::Bounds)?;
		if !output.is_empty() {
			for block in offset.0 / NATIVE_BLOCK_BYTES..=(end - 1) / NATIVE_BLOCK_BYTES {
				if !self.native.0.contains_key(&block) {
					return Err(FixtureError::MissingBlock(block));
				}
			}
		}
		Ok(self.source.read_exact_at(offset, output)?)
	}
}

struct LogicalImage<'fixture> {
	native: &'fixture NativeBlocks,
	header: [u8; SECTOR_BYTES],
	cursor: u64,
}

impl Read for LogicalImage<'_> {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		let count = (output.len() as u64).min(DISK_BYTES - self.cursor) as usize;
		if count == 0 {
			return Ok(0);
		}
		let first = self.cursor;
		let end = first + count as u64;
		let output = &mut output[..count];
		output.fill(0);
		copy_overlap(output, first, 0, &self.header);
		if end > APFS_BEGIN_BYTES {
			let start_block = first.saturating_sub(APFS_BEGIN_BYTES) / NATIVE_BLOCK_BYTES;
			let end_block = (end - APFS_BEGIN_BYTES).div_ceil(NATIVE_BLOCK_BYTES);
			for (&block, &data) in self.native.0.range(start_block..end_block) {
				copy_overlap(
					output,
					first,
					APFS_BEGIN_BYTES + block * NATIVE_BLOCK_BYTES,
					data,
				);
			}
		}
		self.cursor = end;
		Ok(count)
	}
}

fn copy_overlap(output: &mut [u8], output_start: u64, data_start: u64, data: &[u8]) {
	let start = output_start.max(data_start);
	let end = (output_start + output.len() as u64).min(data_start + data.len() as u64);
	if start < end {
		output[(start - output_start) as usize..(end - output_start) as usize]
			.copy_from_slice(&data[(start - data_start) as usize..(end - data_start) as usize]);
	}
}

fn mbr() -> [u8; SECTOR_BYTES] {
	let mut header = [0; SECTOR_BYTES];
	header[MBR_TYPE_OFFSET] = NON_DOS_DATA_PARTITION;
	header[MBR_START_LBA..MBR_START_LBA + size_of::<u32>()]
		.copy_from_slice(&APFS_BEGIN_SECTOR.to_le_bytes());
	let length = (NATIVE_VOLUME_BYTES / SECTOR_BYTES as u64) as u32;
	header[MBR_LENGTH_LBA..MBR_LENGTH_LBA + size_of::<u32>()]
		.copy_from_slice(&length.to_le_bytes());
	header[MBR_SIGNATURE_OFFSET..].copy_from_slice(&MBR_SIGNATURE);
	header
}
