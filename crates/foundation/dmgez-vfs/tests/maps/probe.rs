use std::num::NonZeroU64;

use dmgez_vfs::Filesystem;

use super::*;

#[test]
fn caller_probe_admits_only_its_validated_extent() {
	const EXTENT_BYTES: u64 = SECTOR_BYTES;
	let image = [0; 2 * SECTOR];
	let mut disk = Disk::discover(Raw(image.as_slice()), Limits::default()).unwrap();
	disk.recognize_partitionless(Filesystem::Apfs, |mut window| {
		assert_eq!(window.len(), image.len() as u64);
		let mut bytes = [0; SECTOR];
		window.read_exact_at(0, &mut bytes)?;
		assert_eq!(bytes, EMPTY);
		Ok::<_, Error>(NonZeroU64::new(EXTENT_BYTES))
	})
	.unwrap();
	assert_eq!(
		disk.layout(),
		&Layout::Partitionless {
			filesystem: Filesystem::Apfs,
			range: 0..EXTENT_BYTES,
		}
	);
	let selected = disk.select_volume(None).unwrap();
	assert_eq!(selected, VolumeId::WholeDisk);
	let mut window = disk.data_window(selected).unwrap();
	assert!(matches!(
		window.read_exact_at(EXTENT_BYTES, &mut [0]),
		Err(Error::OutOfRange { .. })
	));
}

#[test]
fn absent_failed_or_unbounded_probe_cannot_admit_a_volume() {
	let mut disk = Disk::discover(Raw(EMPTY.as_slice()), Limits::default()).unwrap();
	disk.recognize_partitionless(Filesystem::Apfs, |_| Ok::<_, Error>(None))
		.unwrap();
	assert_eq!(disk.layout(), &Layout::Unknown);
	assert!(matches!(
		disk.recognize_partitionless(Filesystem::Apfs, |_| Err(Error::Cancelled)),
		Err(Error::Cancelled)
	));
	assert_eq!(disk.layout(), &Layout::Unknown);
	assert!(matches!(
		disk.recognize_partitionless(Filesystem::Apfs, |_| Ok::<_, Error>(Some(NonZeroU64::MAX))),
		Err(Error::OutOfRange { .. })
	));
	assert_eq!(disk.layout(), &Layout::Unknown);
}

#[test]
fn caller_probe_cannot_override_a_partition_map() {
	let mut image = [0; 4 * SECTOR];
	let mut header = EMPTY;
	put_entry(&mut header, 0, APPLE_HFS, 1, 1);
	put_entry(&mut header, 1, APPLE_HFS, 2, 1);
	image[..SECTOR].copy_from_slice(&header);
	let mut disk = Disk::discover(Raw(image.as_slice()), Limits::default()).unwrap();
	disk.recognize_partitionless::<Error>(Filesystem::Apfs, |_| {
		panic!("probe called with a recognized partition map")
	})
	.unwrap();
	assert!(matches!(
		disk.select_volume(None),
		Err(Error::AmbiguousVolumes { candidates: 2 })
	));
}
