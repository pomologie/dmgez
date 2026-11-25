//! Native map observations and adversarial range checks for D-READ-01-T01.

use std::num::NonZeroU32;

use dmgez_udif::SECTOR_BYTES;
use dmgez_vfs::{Disk, Error, Layout, Limits, PartitionId, PartitionRole, Raw, Scheme, VolumeId};

#[path = "maps/access.rs"]
mod access;
#[path = "maps/probe.rs"]
mod probe;

const SECTOR: usize = SECTOR_BYTES as usize;
const EMPTY: [u8; SECTOR] = [0; SECTOR];
const MBR_TABLE_OFFSET: usize = 446;
const MBR_ENTRY_BYTES: usize = 16;
const MBR_CODE: usize = 4;
const MBR_START: usize = 8;
const MBR_COUNT: usize = 12;
const EXTENDED: u8 = 0x0F;
const APPLE_HFS: u8 = 0xAF;
const BOOT_SIGNATURE_OFFSET: usize = 510;
const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];

fn selector(scheme: Scheme, slot: u32) -> VolumeId {
	VolumeId::Partition(PartitionId {
		scheme,
		slot: NonZeroU32::new(slot).unwrap(),
	})
}

#[test]
fn unknown_bytes_do_not_create_a_whole_disk_volume() {
	let mut disk = Disk::discover(Raw(EMPTY.as_slice()), Limits::default()).unwrap();
	assert_eq!(disk.layout(), &Layout::Unknown);
	assert!(matches!(disk.select_volume(None), Err(Error::NoVolume)));
	assert!(matches!(
		disk.select_volume(Some(VolumeId::WholeDisk)),
		Err(Error::UnknownVolume(VolumeId::WholeDisk))
	));
	assert!(matches!(
		disk.window(VolumeId::WholeDisk),
		Err(Error::UnknownVolume(_))
	));
}

#[test]
fn reads_only_the_selected_partition_and_nested_window() {
	let mut image = vec![0x31; 12 * SECTOR];
	let mut zero = EMPTY;
	put_entry(&mut zero, 1, APPLE_HFS, 3, 2);
	image[..SECTOR].copy_from_slice(&zero);
	image[3 * SECTOR..5 * SECTOR].fill(0xCD);
	let mut disk = Disk::discover(Raw(image.as_slice()), Limits::default()).unwrap();
	assert_eq!(disk.select_volume(None).unwrap(), selector(Scheme::Mbr, 2));
	assert!(matches!(
		disk.window(selector(Scheme::Mbr, 1)),
		Err(Error::UnknownVolume(_))
	));
	assert!(matches!(
		disk.window(selector(Scheme::Apple, 2)),
		Err(Error::UnknownVolume(_))
	));
	let mut window = disk.window(selector(Scheme::Mbr, 2)).unwrap();
	assert_eq!(window.len(), 2 * SECTOR_BYTES);
	assert_eq!(window.logical_offset(), 3 * SECTOR_BYTES);
	assert!(matches!(
		window.subwindow(u64::MAX, 2),
		Err(Error::OutOfRange { .. })
	));
	let offset = window.len() - 1;
	assert!(matches!(
		window.read_exact_at(offset, &mut [0; 2]),
		Err(Error::OutOfRange { .. })
	));
	let mut tail = window.subwindow(offset, 1).unwrap();
	let mut bytes = [0];
	tail.read_exact_at(0, &mut bytes).unwrap();
	assert_eq!(bytes, [0xCD]);
	tail.read_exact_at(1, &mut []).unwrap();
	assert!(matches!(
		tail.read_exact_at(1, &mut bytes),
		Err(Error::OutOfRange { .. })
	));
}

#[test]
fn overlapping_mbr_ranges_never_admit_windows() {
	let mut image = [0; 12 * SECTOR];
	let mut header = EMPTY;
	put_entry(&mut header, 0, APPLE_HFS, 3, 4);
	put_entry(&mut header, 1, APPLE_HFS, 6, 4);
	image[..SECTOR].copy_from_slice(&header);
	assert!(matches!(
		Disk::discover(Raw(image.as_slice()), Limits::default()),
		Err(Error::Conflict { .. })
	));
}

#[test]
fn extended_chain_uses_distinct_logical_slots_and_rejects_cycles() {
	let mut image = [0; 64 * SECTOR];
	let mut master = EMPTY;
	put_entry(&mut master, 2, EXTENDED, 10, 40);
	image[..SECTOR].copy_from_slice(&master);
	let mut first_ebr = EMPTY;
	put_entry(&mut first_ebr, 0, APPLE_HFS, 1, 4);
	put_entry(&mut first_ebr, 1, EXTENDED, 10, 30);
	image[10 * SECTOR..11 * SECTOR].copy_from_slice(&first_ebr);
	let mut second_ebr = EMPTY;
	put_entry(&mut second_ebr, 0, APPLE_HFS, 1, 5);
	image[20 * SECTOR..21 * SECTOR].copy_from_slice(&second_ebr);

	let mut disk = Disk::discover(Raw(image.as_slice()), Limits::default()).unwrap();
	let Layout::Partitioned {
		scheme, partitions, ..
	} = disk.layout()
	else {
		panic!("expected MBR");
	};
	assert_eq!(*scheme, Scheme::Mbr);
	assert_eq!(partitions[0].role, PartitionRole::ExtendedContainer);
	assert_eq!(partitions[1].range, 11 * SECTOR_BYTES..15 * SECTOR_BYTES);
	assert_eq!(partitions[2].range, 21 * SECTOR_BYTES..26 * SECTOR_BYTES);
	assert!(matches!(
		disk.select_volume(None),
		Err(Error::AmbiguousVolumes { candidates: 2 })
	));
	let explicit = selector(Scheme::Mbr, 6);
	assert_eq!(disk.select_volume(Some(explicit)).unwrap(), explicit);
	assert!(matches!(
		disk.select_volume(Some(selector(Scheme::Mbr, 3))),
		Err(Error::NoDataWindow(_))
	));
	assert_eq!(
		disk.window(selector(Scheme::Mbr, 5)).unwrap().len(),
		4 * SECTOR_BYTES
	);
	assert_eq!(
		disk.window(selector(Scheme::Mbr, 6)).unwrap().len(),
		5 * SECTOR_BYTES
	);
	assert!(matches!(
		disk.data_window(selector(Scheme::Mbr, 3)),
		Err(Error::NoDataWindow(_))
	));

	let limited = Limits {
		max_entries: 1,
		..Limits::default()
	};
	assert!(matches!(
		Disk::discover(Raw(image.as_slice()), limited),
		Err(Error::Limit(_))
	));
	put_entry(&mut second_ebr, 1, EXTENDED, 0, 40);
	image[20 * SECTOR..21 * SECTOR].copy_from_slice(&second_ebr);
	assert!(matches!(
		Disk::discover(Raw(image.as_slice()), Limits::default()),
		Err(Error::Corrupt {
			scheme: Scheme::Mbr,
			..
		})
	));
}

fn put_entry(sector: &mut [u8; SECTOR], slot: usize, code: u8, start: u32, count: u32) {
	sector[BOOT_SIGNATURE_OFFSET..].copy_from_slice(&MBR_SIGNATURE);
	let offset = MBR_TABLE_OFFSET + slot * MBR_ENTRY_BYTES;
	let entry = &mut sector[offset..][..MBR_ENTRY_BYTES];
	entry[MBR_CODE] = code;
	entry[MBR_START..][..4].copy_from_slice(&start.to_le_bytes());
	entry[MBR_COUNT..][..4].copy_from_slice(&count.to_le_bytes());
}

#[cfg(feature = "zlib")]
mod native {
	use super::*;
	use dmgez_udif::reader::{Image, Limits as UdifLimits};
	use dmgez_vfs::{Filesystem, PartitionKind};

	const HFS_HEADER_OFFSET: u64 = 1024;
	const HFS_PLUS_HEADER: [u8; 4] = [0x48, 0x2B, 0, 4];
	const APM: &[u8] = include_bytes!("fixtures/native-apm.dmg");
	const DUAL_APM: &[u8] = include_bytes!("fixtures/native-dual-apm.dmg");
	const GPT: &[u8] = include_bytes!("fixtures/native-gpt.dmg");
	const MBR: &[u8] = include_bytes!("fixtures/native-mbr.dmg");
	const PARTITIONLESS: &[u8] = include_bytes!("fixtures/native-raw.dmg");

	#[test]
	fn native_partitions_match_apple_reported_offsets_and_hold_the_hfs_header() {
		for (fixture, scheme, slot, first_sector, sector_count) in [
			(APM, Scheme::Apple, 2, 64, 32_704),
			(GPT, Scheme::Gpt, 1, 40, 32_688),
			(MBR, Scheme::Mbr, 1, 1, 32_767),
		] {
			let image = Image::open(fixture, UdifLimits::default()).unwrap();
			let mut disk = Disk::discover(image, Limits::default()).unwrap();
			let Layout::Partitioned {
				scheme: found,
				block_bytes,
				partitions,
			} = disk.layout()
			else {
				panic!("expected {scheme}");
			};
			assert_eq!(*found, scheme);
			assert_eq!(*block_bytes, SECTOR_BYTES);
			let selected = selector(scheme, slot);
			let entry = partitions
				.iter()
				.find(|entry| VolumeId::Partition(entry.id) == selected)
				.unwrap();
			assert_eq!(
				entry.range,
				first_sector * SECTOR_BYTES..(first_sector + sector_count) * SECTOR_BYTES
			);
			assert_eq!(entry.role, PartitionRole::Data);
			if scheme == Scheme::Apple {
				assert_eq!(partitions[0].role, PartitionRole::Metadata);
				assert!(matches!(partitions[0].kind, PartitionKind::Apple { .. }));
			}
			let mut window = disk.data_window(selected).unwrap();
			let mut header = [0; 4];
			window
				.read_exact_at(HFS_HEADER_OFFSET, &mut header)
				.unwrap();
			assert_eq!(header, HFS_PLUS_HEADER);
		}
	}

	#[test]
	fn native_optical_dual_apm_requires_an_explicit_supported_profile() {
		let image = Image::open(DUAL_APM, UdifLimits::default()).unwrap();
		assert!(matches!(
			Disk::discover(image, Limits::default()),
			Err(Error::Unsupported {
				scheme: Scheme::Apple,
				..
			})
		));
	}

	#[test]
	fn partitionless_volume_uses_header_geometry() {
		let mut image = Image::open(PARTITIONLESS, UdifLimits::default()).unwrap();
		let mut disk = Disk::discover(&mut image, Limits::default()).unwrap();
		let Layout::Partitionless { filesystem, range } = disk.layout() else {
			panic!("expected HFS+");
		};
		assert_eq!(*filesystem, Filesystem::HfsPlus);
		assert_eq!(*range, 0..16 * (1 << 20));
		let mut header = [0; 4];
		disk.window(VolumeId::WholeDisk)
			.unwrap()
			.read_exact_at(HFS_HEADER_OFFSET, &mut header)
			.unwrap();
		assert_eq!(header, HFS_PLUS_HEADER);
	}

	#[test]
	fn invalid_gpt_header_backup_array_and_hybrid_mbr_refuse_access() {
		let mut raw = decode(GPT);
		let backup_header = raw.len() - SECTOR;
		let crc_field = 16;
		raw[backup_header + crc_field] ^= 1;
		assert_corrupt(&raw, Scheme::Gpt);
		raw[backup_header + crc_field] ^= 1;
		let backup_array = backup_header - 32 * SECTOR;
		raw[backup_array] ^= 1;
		assert_corrupt(&raw, Scheme::Gpt);
		raw[backup_array] ^= 1;
		let master: &mut [u8; SECTOR] = (&mut raw[..SECTOR]).try_into().unwrap();
		put_entry(master, 1, APPLE_HFS, 40, 1);
		assert!(matches!(
			Disk::discover(Raw(raw.as_slice()), Limits::default()),
			Err(Error::Conflict { .. })
		));
	}

	#[test]
	fn unsupported_gpt_entry_size_is_refused_before_upstream_decoder() {
		const HEADER_BYTES: usize = 92;
		const STRIDE: usize = 84;
		const CRC: usize = 16;
		let mut raw = decode(GPT);
		let header = &mut raw[SECTOR..SECTOR + HEADER_BYTES];
		header[STRIDE..][..4].copy_from_slice(&256u32.to_le_bytes());
		header[CRC..][..4].fill(0);
		let crc = crc32fast::hash(header);
		header[CRC..][..4].copy_from_slice(&crc.to_le_bytes());
		assert!(matches!(
			Disk::discover(Raw(raw.as_slice()), Limits::default()),
			Err(Error::Unsupported {
				scheme: Scheme::Gpt,
				..
			})
		));
	}

	#[test]
	fn gpt_with_4096_byte_logical_blocks_has_byte_exact_windows() {
		const BLOCK: usize = 4096;
		const DISK_BLOCKS: usize = 128;
		const FINAL: usize = DISK_BLOCKS - 1;
		const ENTRY_ARRAY_BYTES: usize = 16 << 10;
		const BACKUP_ARRAY_BLOCK: usize = FINAL - ENTRY_ARRAY_BYTES / BLOCK;
		const FIRST_USABLE: u64 = 6;
		const LAST_USABLE: u64 = BACKUP_ARRAY_BLOCK as u64 - 1;
		const PARTITION_FIRST: u64 = 16;
		const PARTITION_LAST: u64 = 31;
		const PROTECTIVE: u8 = 0xEE;
		const ARRAY_LBA: usize = 2;
		const GPT_FIRST_LBA_OFFSET: usize = 32;
		const GPT_LAST_LBA_OFFSET: usize = 40;
		const HEADER_BYTES: usize = 92;
		const HEADER_CRC: usize = 16;
		const HEADER_CURRENT_LBA: usize = 24;
		const HEADER_OTHER_LBA: usize = 32;
		const HEADER_FIRST_USABLE: usize = 40;
		const HEADER_LAST_USABLE: usize = 48;
		const HEADER_ARRAY_LBA: usize = 72;
		const HEADER_ARRAY_CRC: usize = 88;

		let template = decode(GPT);
		let start = ARRAY_LBA * SECTOR;
		let mut entry_array = template[start..start + ENTRY_ARRAY_BYTES].to_vec();
		put_le64(&mut entry_array, GPT_FIRST_LBA_OFFSET, PARTITION_FIRST);
		put_le64(&mut entry_array, GPT_LAST_LBA_OFFSET, PARTITION_LAST);
		let array_crc = crc32fast::hash(&entry_array);
		let mut image = vec![0; DISK_BLOCKS * BLOCK];
		let mut protective = EMPTY;
		put_entry(&mut protective, 0, PROTECTIVE, 1, FINAL as u32);
		image[..SECTOR].copy_from_slice(&protective);
		for (current, other, array) in [(1, FINAL, ARRAY_LBA), (FINAL, 1, BACKUP_ARRAY_BLOCK)] {
			let offset = current * BLOCK;
			let header = &mut image[offset..offset + HEADER_BYTES];
			header.copy_from_slice(&template[SECTOR..SECTOR + HEADER_BYTES]);
			put_le64(header, HEADER_CURRENT_LBA, current as u64);
			put_le64(header, HEADER_OTHER_LBA, other as u64);
			put_le64(header, HEADER_FIRST_USABLE, FIRST_USABLE);
			put_le64(header, HEADER_LAST_USABLE, LAST_USABLE);
			put_le64(header, HEADER_ARRAY_LBA, array as u64);
			header[HEADER_ARRAY_CRC..][..4].copy_from_slice(&array_crc.to_le_bytes());
			header[HEADER_CRC..][..4].fill(0);
			let header_crc = crc32fast::hash(header);
			header[HEADER_CRC..][..4].copy_from_slice(&header_crc.to_le_bytes());
			let offset = array * BLOCK;
			image[offset..offset + ENTRY_ARRAY_BYTES].copy_from_slice(&entry_array);
		}
		let payload = b"4K logical blocks";
		let start = PARTITION_FIRST as usize * BLOCK;
		image[start..start + payload.len()].copy_from_slice(payload);
		let mut disk = Disk::discover(Raw(image.as_slice()), Limits::default()).unwrap();
		assert!(matches!(
			disk.layout(),
			Layout::Partitioned {
				scheme: Scheme::Gpt,
				block_bytes: 4096,
				..
			}
		));
		let mut window = disk.window(selector(Scheme::Gpt, 1)).unwrap();
		assert_eq!(window.logical_offset(), PARTITION_FIRST * BLOCK as u64);
		assert_eq!(
			window.len(),
			(PARTITION_LAST - PARTITION_FIRST + 1) * BLOCK as u64
		);
		let mut output = [0; 17];
		window.read_exact_at(0, &mut output).unwrap();
		assert_eq!(&output, payload);
	}

	fn put_le64(bytes: &mut [u8], offset: usize, value: u64) {
		bytes[offset..][..8].copy_from_slice(&value.to_le_bytes());
	}

	#[test]
	fn apm_map_count_and_data_subrange_are_validated() {
		const COUNT_FIELD: usize = 4;
		const DATA_OFFSET_FIELD: usize = 80;
		let mut raw = decode(APM);
		let second_map_sector = SECTOR * 2;
		let count_offset = second_map_sector + COUNT_FIELD;
		let old_count: [u8; 4] = raw[count_offset..][..4].try_into().unwrap();
		raw[count_offset..][..4].copy_from_slice(&1u32.to_be_bytes());
		assert_corrupt(&raw, Scheme::Apple);
		raw[count_offset..][..4].copy_from_slice(&old_count);
		raw[second_map_sector + DATA_OFFSET_FIELD..][..4].copy_from_slice(&u32::MAX.to_be_bytes());
		assert_corrupt(&raw, Scheme::Apple);
	}

	#[test]
	fn gpt_tables_and_apm_records_obey_caller_budgets() {
		let image = Image::open(GPT, UdifLimits::default()).unwrap();
		assert!(matches!(
			Disk::discover(
				image,
				Limits {
					max_probe_bytes: 4096,
					..Limits::default()
				}
			),
			Err(Error::Limit(_))
		));
		let image = Image::open(APM, UdifLimits::default()).unwrap();
		assert!(matches!(
			Disk::discover(
				image,
				Limits {
					max_entries: 1,
					..Limits::default()
				}
			),
			Err(Error::Limit(_))
		));
	}

	fn decode(fixture: &[u8]) -> Vec<u8> {
		let mut image = Image::open(fixture, UdifLimits::default()).unwrap();
		let mut raw = Vec::new();
		image.write_raw_to(&mut raw).unwrap();
		raw
	}

	fn assert_corrupt(raw: &[u8], scheme: Scheme) {
		assert!(
			matches!(Disk::discover(Raw(raw), Limits::default()), Err(Error::Corrupt { scheme: observed, .. }) if observed == scheme)
		);
	}
}
