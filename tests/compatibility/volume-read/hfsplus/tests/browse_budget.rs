use std::{cell::RefCell, num::NonZeroU16, ops::Range};

use dmgez_hfsplus_read::browse::{Error, Limits, SourceError, with_catalog, with_view};
use dmgez_vfs::{
	Disk, LogicalDisk, VolumeId,
	volume::{ForkKind, LookupName, NodeKind},
};
use hfsplus_core::{ByteOffset, ReadAt};
use hfsplus_format::volume::{HEADER_BYTES, PRIMARY_HEADER_OFFSET, Signature};
use hfsplus_reader::overflow::ForkKind as StoredFork;

const FIXTURE: &[u8] = include_bytes!("fixtures/native-chained-hardlinks.bin");
const SOURCE_BYTES: u64 = 16 << 30;
const NATIVE_VOLUME_BYTES: u64 = 16 << 20;
const ALLOCATION_BLOCK_BYTES: u64 = 4096;
const MAX_METADATA_BYTES: u64 = 256 << 10;
const NATIVE_PAYLOAD_BLOCKS: [u64; 3] = [INODE_19_DATA, INODE_23_DATA, INODE_19_RESOURCE];
const LINK_NAME: &str = "one";
const NATIVE_LINK_COUNT: u32 = 3;

// Apple-native fixture ranges from hfsplus-reader/tests/fixtures/native-hardlink-resolution.json.
const INODE_19_DATA: u64 = 3_022_848;
const INODE_23_DATA: u64 = 3_026_944;
const INODE_19_RESOURCE: u64 = 3_031_040;
const NATIVE_DATA: &[u8] = b"pomologie linked file\n";
const NATIVE_RESOURCE: &[u8] = b"pomologie resource\0fork\n";

type Reads = RefCell<Vec<Range<u64>>>;

struct SparseNativeDisk<'a> {
	slices: Vec<(Range<u64>, &'static [u8])>,
	reads: &'a Reads,
}

impl LogicalDisk for SparseNativeDisk<'_> {
	fn len(&self) -> u64 {
		SOURCE_BYTES
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), dmgez_udif::Error> {
		let len = output.len() as u64;
		let end = offset
			.checked_add(len)
			.filter(|&end| end <= self.len())
			.ok_or(dmgez_udif::Error::OutOfRange {
				offset,
				len,
				disk_len: self.len(),
			})?;
		self.reads.borrow_mut().push(offset..end);
		output.fill(0);
		for (capture, bytes) in &self.slices {
			let start = offset.max(capture.start);
			let finish = end.min(capture.end);
			if start < finish {
				let from = usize::try_from(start - capture.start).unwrap();
				let to = usize::try_from(start - offset).unwrap();
				let count = usize::try_from(finish - start).unwrap();
				output[to..to + count].copy_from_slice(&bytes[from..from + count]);
			}
		}
		Ok(())
	}
}

fn native_disk(reads: &Reads) -> Disk<SparseNativeDisk<'_>> {
	fn next<const N: usize>(input: &mut &'static [u8]) -> [u8; N] {
		let (field, remaining) = input.split_first_chunk().unwrap();
		*input = remaining;
		*field
	}
	let mut fixture = FIXTURE;
	assert_eq!(u64::from_be_bytes(next(&mut fixture)), NATIVE_VOLUME_BYTES);
	let count = u16::from_be_bytes(next(&mut fixture));
	let mut slices = Vec::with_capacity(usize::from(count));
	for _ in 0..count {
		let offset = u64::from_be_bytes(next(&mut fixture));
		let length = u32::from_be_bytes(next(&mut fixture));
		let (capture, remaining) = fixture.split_at(usize::try_from(length).unwrap());
		slices.push((offset..offset + u64::from(length), capture));
		fixture = remaining;
	}
	assert!(fixture.is_empty());
	let source = SparseNativeDisk { slices, reads };
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).unwrap();
	assert_eq!(disk.select_volume(None).unwrap(), VolumeId::WholeDisk);
	assert_eq!(
		disk.data_window(VolumeId::WholeDisk).unwrap().len(),
		NATIVE_VOLUME_BYTES
	);
	disk
}

fn payload_reads(reads: &[Range<u64>]) -> Vec<Range<u64>> {
	reads
		.iter()
		.filter(|read| {
			!read.is_empty()
				&& NATIVE_PAYLOAD_BLOCKS
					.iter()
					.any(|&start| read.start < start + ALLOCATION_BLOCK_BYTES && start < read.end)
		})
		.cloned()
		.collect()
}

#[test]
fn directory_and_hard_link_stat_never_read_native_payload_blocks() {
	let reads = Reads::default();
	let mut disk = native_disk(&reads);
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		{
			let mut reader = view.read_dir(&root).unwrap();
			let first = reader.next_page(NonZeroU16::MIN).unwrap();
			assert_eq!(first.entries.len(), 1);
			assert!(!first.complete);
		}
		let linked = view
			.lookup(&root, LookupName::Utf8(LINK_NAME))
			.unwrap()
			.unwrap();
		let metadata = view.stat(&linked.node).unwrap();
		assert_eq!(metadata.kind, NodeKind::RegularFile);
		assert_eq!(metadata.hard_link.unwrap().count.get(), NATIVE_LINK_COUNT);
	})
	.unwrap();
	let reads = reads.borrow();
	assert!(payload_reads(&reads).is_empty(), "source reads: {reads:?}");
	let total = reads
		.iter()
		.map(|range| range.end - range.start)
		.sum::<u64>();
	assert!(total <= MAX_METADATA_BYTES, "read {total} bytes");
}

#[test]
fn exact_data_and_resource_reads_touch_only_their_requested_native_ranges() {
	const START: usize = 4;
	const LENGTH: usize = 7;
	let reads = Reads::default();
	let mut disk = native_disk(&reads);
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		let entry = view
			.lookup(&root, LookupName::Utf8(LINK_NAME))
			.unwrap()
			.unwrap();
		for (kind, physical_start, expected) in [
			(ForkKind::Data, INODE_19_DATA, NATIVE_DATA),
			(ForkKind::Resource, INODE_19_RESOURCE, NATIVE_RESOURCE),
		] {
			reads.borrow_mut().clear();
			view.with_fork(&entry.node, kind, |fork| {
				assert_eq!(fork.len(), expected.len() as u64);
				assert!(payload_reads(&reads.borrow()).is_empty());
				let mut actual = [0; LENGTH];
				fork.read_exact_at(START as u64, &mut actual).unwrap();
				assert_eq!(actual, expected[START..START + LENGTH]);
			})
			.unwrap()
			.unwrap();
			let start = physical_start + START as u64;
			let payload = payload_reads(&reads.borrow());
			assert_eq!(payload.len(), 1, "payload reads: {payload:?}");
			assert_eq!(payload[0], start..start + LENGTH as u64);
		}
	})
	.unwrap();
}

#[test]
fn native_catalog_lends_both_stored_forks_without_materializing_payload() {
	let reads = Reads::default();
	let mut disk = native_disk(&reads);
	let limits = Limits::default();
	with_catalog(&mut disk, VolumeId::WholeDisk, limits, |volume, catalog| {
		assert_eq!(volume.header().signature, Signature::HfsPlus);
		let root = catalog.root().unwrap();
		let entry = catalog
			.lookup(root.metadata.id, LINK_NAME)
			.unwrap()
			.unwrap();
		let file = catalog
			.regular_file(entry.metadata.id, limits.hard_links)
			.unwrap()
			.unwrap();
		assert_eq!(file.hard_link().unwrap().count.get(), NATIVE_LINK_COUNT);
		let data = file
			.open_stored_fork(StoredFork::Data, limits.stored_fork)
			.unwrap();
		let resource = file
			.open_stored_fork(StoredFork::Resource, limits.stored_fork)
			.unwrap();
		assert!(payload_reads(&reads.borrow()).is_empty());
		for (fork, start, expected) in [
			(data, INODE_19_DATA, NATIVE_DATA),
			(resource, INODE_19_RESOURCE, NATIVE_RESOURCE),
		] {
			reads.borrow_mut().clear();
			assert_eq!(fork.len().0, expected.len() as u64);
			let mut output = vec![0; expected.len()];
			fork.read_exact_at(ByteOffset(0), &mut output).unwrap();
			assert_eq!(output, expected);
			let payload = payload_reads(&reads.borrow());
			assert_eq!(payload.len(), 1);
			assert_eq!(payload[0], start..start + fork.len().0);
		}
	})
	.unwrap();
}

#[test]
fn volume_read_budgets_stop_after_the_first_header_before_dispatching_another_read() {
	for (max_read_requests, max_read_bytes) in [(1, u64::MAX), (u64::MAX, HEADER_BYTES as u64)] {
		let reads = Reads::default();
		let mut disk = native_disk(&reads);
		reads.borrow_mut().clear();
		let limits = Limits {
			max_read_requests,
			max_read_bytes,
			..Limits::default()
		};
		let result: Result<(), _> = with_view(&mut disk, VolumeId::WholeDisk, limits, |_| {
			panic!("read budget failed to stop volume admission")
		});
		let Err(Error::Volume(hfsplus_reader::Error::Source { error, .. })) = result else {
			panic!("expected typed read budget error, got {result:?}");
		};
		if max_read_requests == 1 {
			assert!(matches!(error, SourceError::RequestLimit));
		} else {
			assert!(matches!(error, SourceError::ByteLimit));
		}
		let reads = reads.borrow();
		assert_eq!(reads.len(), 1, "source reads: {reads:?}");
		assert_eq!(
			reads[0],
			PRIMARY_HEADER_OFFSET..PRIMARY_HEADER_OFFSET + HEADER_BYTES as u64
		);
	}
}
