#![cfg(feature = "analysis")]

use std::{env, fs, io::Read, ops::Range, path::PathBuf};

use dmgez_hfsplus_read::{
	analysis::{Error, Format, Limit, Limits, analyze},
	browse,
};
use dmgez_udif::Error as SourceError;
use dmgez_vfs::{Disk, LogicalDisk, Raw, VolumeId};
use hfsplus_reader::{
	Volume,
	allocation::AllocationError,
	catalog::{CatalogLimits, EntryKind},
};

const FIXTURE_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";
const CATALOG_IMAGE: &str = "native-catalog-volume.bin";
const HARDLINK_IMAGE: &str = "native-chained-hardlinks.bin";
const HARDLINK_BITMAP: &str = "native-hardlink-allocation.bin";
const HARDLINK_BITMAP_OFFSET: usize = 4096;
const NESTED_DIRECTORY: &str = "Nested";
const LARGEST_NAME: &str = "Re\u{301}sume\u{301}.txt";
const LARGEST_PAYLOAD: &[u8] = b"catalog unicode payload\n";
const NATIVE_BLOCK_BYTES: u32 = 2048;
const NATIVE_TOTAL_BLOCKS: u32 = 42;
const NATIVE_METADATA_BLOCKS: u32 = 41;
const NATIVE_REGULAR_FILES: u64 = 97;
const HARDLINK_FIRST_INODE: u32 = 19;
const HARDLINK_SECOND_INODE: u32 = 23;
const HARDLINK_FIRST_BYTES: u64 = 22;
const HARDLINK_SECOND_BYTES: u64 = 23;

#[test]
fn checks_native_allocation_and_ranks_known_sizes_without_reading_the_payload() {
	let bytes = fixture(CATALOG_IMAGE);
	let native = Volume::open(bytes.as_slice()).unwrap();
	let catalog = native.open_catalog(CatalogLimits::default()).unwrap();
	let root = catalog.root().unwrap();
	let nested = catalog
		.lookup(root.metadata.id, NESTED_DIRECTORY)
		.unwrap()
		.unwrap();
	let largest = catalog
		.lookup(nested.metadata.id, LARGEST_NAME)
		.unwrap()
		.unwrap();
	let EntryKind::File { data, .. } = largest.kind else {
		panic!("the test fixture's largest entry is not a file");
	};
	let extent = data.extents().next().unwrap();
	let payload = native
		.header()
		.geometry
		.block_range(extent.start, extent.blocks)
		.unwrap();
	let source = NoPayloadReads {
		bytes: &bytes,
		payload,
	};
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).unwrap();
	let mut limits = Limits::default();
	limits.largest_files = 3;
	let report = analyze(&mut disk, VolumeId::WholeDisk, limits).unwrap();

	assert_eq!(report.format, Format::HfsPlus);
	assert_eq!(report.allocation.block_size, NATIVE_BLOCK_BYTES);
	assert_eq!(report.allocation.total_blocks, NATIVE_TOTAL_BLOCKS);
	assert_eq!(report.allocation.free_blocks, 0);
	assert_eq!(report.allocation.metadata_blocks, NATIVE_METADATA_BLOCKS);
	assert_eq!(report.allocation.other_allocated_blocks, 1);
	assert_eq!(report.content.directories, 1);
	assert_eq!(report.content.regular_files, NATIVE_REGULAR_FILES);
	assert_eq!(report.content.sized_files, NATIVE_REGULAR_FILES);
	assert!(report.content.complete);
	let [first, tied_first, tied_second] = report.content.largest_files.as_slice() else {
		panic!("analysis did not retain the requested number of ranked paths");
	};
	assert_eq!(first.logical_bytes, LARGEST_PAYLOAD.len() as u64);
	assert_eq!(
		first.path,
		[
			NESTED_DIRECTORY.encode_utf16().collect::<Vec<_>>(),
			LARGEST_NAME.encode_utf16().collect(),
		]
	);
	assert_eq!(tied_first.logical_bytes, 0);
	assert_eq!(tied_second.logical_bytes, 0);
	assert!(tied_first.catalog_id < tied_second.catalog_id);
}

#[test]
fn reports_actual_entry_and_depth_truncation() {
	let bytes = fixture(CATALOG_IMAGE);
	let mut limits = Limits::default();
	limits.max_entries = 1;
	let mut disk = Disk::discover(Raw(bytes.as_slice()), dmgez_vfs::Limits::default()).unwrap();
	let report = analyze(&mut disk, VolumeId::WholeDisk, limits).unwrap();
	assert_eq!(report.content.entries, 1);
	assert_eq!(report.content.limits_reached, [Limit::Entries]);
	assert!(!report.content.complete);

	limits.max_entries = NATIVE_REGULAR_FILES + 1;
	limits.max_depth = 1;
	let report = analyze(&mut disk, VolumeId::WholeDisk, limits).unwrap();
	assert_eq!(report.content.entries, NATIVE_REGULAR_FILES);
	assert_eq!(report.content.limits_reached, [Limit::Depth]);
	assert!(!report.content.complete);

	limits.max_depth = Limits::default().max_depth;
	let report = analyze(&mut disk, VolumeId::WholeDisk, limits).unwrap();
	assert_eq!(report.content.entries, limits.max_entries);
	assert!(report.content.limits_reached.is_empty());
	assert!(report.content.complete);
}

#[test]
fn reports_hard_link_paths_with_shared_storage_ids() {
	let slices = fixture(HARDLINK_IMAGE);
	let mut reader = slices.as_slice();
	let total = u64::from_be_bytes(piece(&mut reader));
	let count = u16::from_be_bytes(piece(&mut reader));
	let mut bytes = vec![0; usize::try_from(total).unwrap()];
	for _ in 0..count {
		let offset = usize::try_from(u64::from_be_bytes(piece(&mut reader))).unwrap();
		let len = u32::from_be_bytes(piece(&mut reader)) as usize;
		reader.read_exact(&mut bytes[offset..offset + len]).unwrap();
	}
	assert!(reader.is_empty());
	let bitmap = fixture(HARDLINK_BITMAP);
	bytes[HARDLINK_BITMAP_OFFSET..][..bitmap.len()].copy_from_slice(&bitmap);
	let mut disk = Disk::discover(Raw(bytes.as_slice()), dmgez_vfs::Limits::default()).unwrap();
	let report = analyze(&mut disk, VolumeId::WholeDisk, Limits::default()).unwrap();

	assert!(report.content.complete);
	assert_eq!(report.content.entries, 6);
	assert_eq!(report.content.regular_files, 5);
	let paths: Vec<_> = report
		.content
		.largest_files
		.iter()
		.map(|file| {
			(
				file.display_path().to_string(),
				file.inode_id,
				file.logical_bytes,
			)
		})
		.collect();
	assert!(paths.contains(&(
		"/nested/three".into(),
		HARDLINK_FIRST_INODE,
		HARDLINK_FIRST_BYTES
	)));
	assert!(paths.contains(&(
		"/nested/five".into(),
		HARDLINK_SECOND_INODE,
		HARDLINK_SECOND_BYTES
	)));
	assert_eq!(
		paths
			.iter()
			.filter(|(_, inode, _)| *inode == HARDLINK_FIRST_INODE)
			.count(),
		3
	);
	assert_eq!(
		paths
			.iter()
			.filter(|(_, inode, _)| *inode == HARDLINK_SECOND_INODE)
			.count(),
		2
	);
	assert_eq!(paths[0].2, HARDLINK_SECOND_BYTES);
}

#[test]
fn distinguishes_an_allocation_budget_failure() {
	let bytes = fixture(CATALOG_IMAGE);
	let mut disk = Disk::discover(Raw(bytes.as_slice()), dmgez_vfs::Limits::default()).unwrap();
	let mut limits = Limits::default();
	limits.allocation.max_bitmap_bytes = 0;
	assert!(matches!(
		analyze(&mut disk, VolumeId::WholeDisk, limits),
		Err(Error::Allocation(AllocationError::BitmapLimit {
			limit: 0,
			..
		}))
	));
}

#[test]
fn preserves_the_selected_volume_read_budget_failure() {
	let bytes = fixture(CATALOG_IMAGE);
	let mut disk = Disk::discover(Raw(bytes.as_slice()), dmgez_vfs::Limits::default()).unwrap();
	let mut limits = Limits::default();
	limits.source.max_read_requests = 0;
	assert!(matches!(
		analyze(&mut disk, VolumeId::WholeDisk, limits),
		Err(Error::Admission(browse::Error::Volume(
			hfsplus_reader::Error::Source {
				error: browse::SourceError::RequestLimit,
				..
			}
		)))
	));
}

fn fixture(name: &str) -> Vec<u8> {
	let fixtures = PathBuf::from(env::var_os(FIXTURE_ENV).expect("set DMGEZ_HFSPLUS_FIXTURES"));
	fs::read(fixtures.join(name)).unwrap()
}

fn piece<const N: usize>(reader: &mut impl Read) -> [u8; N] {
	let mut bytes = [0; N];
	reader.read_exact(&mut bytes).unwrap();
	bytes
}

struct NoPayloadReads<'bytes> {
	bytes: &'bytes [u8],
	payload: Range<u64>,
}

impl LogicalDisk for NoPayloadReads<'_> {
	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		let end = offset + output.len() as u64;
		assert!(
			end <= self.payload.start || offset >= self.payload.end,
			"read file payload"
		);
		Raw(self.bytes).read_exact_at(offset, output)
	}
}
