#![cfg(feature = "image-edit")]

use std::{fs, path::Path};

use dmgez_operations::edit::image_source::{
	ImageRevision, ImageSource, ImageSourceError, ImageSourceOptions, SourceEnvelope,
};
use dmgez_udif::{
	Error as UdifError, SECTOR_BYTES, TRAILER_BYTES,
	writer::{Profile, WriteOptions, write_image},
};
use dmgez_vfs::{Filesystem, LogicalDisk};
use sha2::{Digest, Sha256};

const PROBE_BLOCK_BYTES: u32 = 4096;
const PROBE_BLOCK_COUNT: u32 = 2;
const VOLUME_HEADER_OFFSET: usize = 1024;
const HFS_PLUS_SIGNATURE_AND_VERSION: [u8; 4] = *b"H+\x00\x04";
const HFS_X_SIGNATURE_AND_VERSION: [u8; 4] = *b"HX\x00\x05";
const BLOCK_BYTES_FIELD: usize = VOLUME_HEADER_OFFSET + 40;
const BLOCK_COUNT_FIELD: usize = VOLUME_HEADER_OFFSET + 44;
const XML_LENGTH_FIELD: std::ops::Range<usize> = 224..232;

// A synthetic header and stable logical bytes. These admission tests exercise
// geometry only. A filesystem-level rebuild must also qualify the catalog.
fn raw_disk() -> Vec<u8> {
	raw_disk_with_signature(HFS_PLUS_SIGNATURE_AND_VERSION)
}

fn raw_disk_with_signature(signature: [u8; 4]) -> Vec<u8> {
	let mut raw = vec![0; (PROBE_BLOCK_BYTES * PROBE_BLOCK_COUNT) as usize];
	raw[VOLUME_HEADER_OFFSET..][..signature.len()].copy_from_slice(&signature);
	raw[BLOCK_BYTES_FIELD..][..4].copy_from_slice(&PROBE_BLOCK_BYTES.to_be_bytes());
	raw[BLOCK_COUNT_FIELD..][..4].copy_from_slice(&PROBE_BLOCK_COUNT.to_be_bytes());
	*raw.last_mut().unwrap() = 1;
	raw
}

fn wrap(raw: &[u8], profile: Profile) -> Vec<u8> {
	let mut bytes = Vec::new();
	let options = WriteOptions {
		profile,
		..WriteOptions::default()
	};
	write_image(raw, raw.len() as u64, &mut bytes, &options).unwrap();
	bytes
}

fn open(path: &Path) -> Result<ImageSource, ImageSourceError> {
	ImageSource::open(path, None, ImageSourceOptions::default())
}

#[test]
fn pins_unsigned_images_to_exact_bytes_and_an_admitted_layout() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("input.dmg");
	for (signature, filesystem) in [
		(HFS_PLUS_SIGNATURE_AND_VERSION, Filesystem::HfsPlus),
		(HFS_X_SIGNATURE_AND_VERSION, Filesystem::HfsX),
	] {
		let raw = raw_disk_with_signature(signature);
		for profile in [Profile::Udro, Profile::Udzo] {
			let container = wrap(&raw, profile);
			fs::write(&path, &container).unwrap();
			let expected = ImageRevision::from_sha256(Sha256::digest(&container).into());
			let mut source =
				ImageSource::open(&path, Some(expected), ImageSourceOptions::default()).unwrap();
			assert_eq!(source.revision(), expected);
			assert_eq!(source.envelope(), SourceEnvelope::Unsigned);
			assert_eq!(source.filesystem(), filesystem);
			assert_eq!(source.container_metadata().source_profile(), profile);
			let mut decoded = vec![0; raw.len()];
			source.read_exact_at(0, &mut decoded).unwrap();
			assert_eq!(decoded, raw);
			source.revalidate().unwrap();
			assert_eq!(fs::read(&path).unwrap(), container);
		}
	}
	assert!(matches!(
		ImageSource::open(
			&path,
			Some(ImageRevision::from_sha256([0; 32])),
			ImageSourceOptions::default()
		),
		Err(ImageSourceError::RevisionMismatch)
	));
}

#[test]
fn refuses_unqualified_geometry_corruption_opaque_metadata_and_limits() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("input.dmg");
	let raw = raw_disk();
	let qualified = wrap(&raw, Profile::Udro);
	fs::write(&path, &qualified).unwrap();
	let mut options = ImageSourceOptions::default();
	options.container.max_physical_bytes = qualified.len() as u64 - 1;
	assert!(matches!(
		ImageSource::open(&path, None, options),
		Err(ImageSourceError::Container(UdifError::ResourceLimit(_)))
	));
	let options = ImageSourceOptions::default();
	options.cancellation.cancel();
	assert!(matches!(
		ImageSource::open(&path, None, options),
		Err(ImageSourceError::Container(UdifError::Cancelled))
	));
	let mut damaged = qualified.clone();
	damaged[raw.len() - 1] ^= 1;
	fs::write(&path, damaged).unwrap();
	assert!(matches!(
		open(&path),
		Err(ImageSourceError::Container(
			UdifError::ChecksumMismatch { .. }
		))
	));
	let mut opaque = qualified
		.split_at(qualified.len() - TRAILER_BYTES)
		.0
		.to_vec();
	let mut trailer = *qualified.last_chunk::<TRAILER_BYTES>().unwrap();
	let mut resources = plist::Value::from_reader_xml(&opaque[raw.len()..]).unwrap();
	resources
		.as_dictionary_mut()
		.unwrap()
		.insert("opaque".into(), plist::Value::Data(vec![1]));
	opaque.truncate(raw.len());
	resources.to_writer_xml(&mut opaque).unwrap();
	trailer[XML_LENGTH_FIELD].copy_from_slice(&((opaque.len() - raw.len()) as u64).to_be_bytes());
	opaque.extend_from_slice(&trailer);
	fs::write(&path, opaque).unwrap();
	assert!(matches!(
		open(&path),
		Err(ImageSourceError::Container(UdifError::Unsupported(_)))
	));
	let mut trailing = raw;
	trailing.resize(trailing.len() + SECTOR_BYTES as usize, 0);
	fs::write(&path, wrap(&trailing, Profile::Udro)).unwrap();
	assert!(matches!(
		open(&path),
		Err(ImageSourceError::NotWholeHfsPlus)
	));
	trailing[VOLUME_HEADER_OFFSET..][..HFS_X_SIGNATURE_AND_VERSION.len()]
		.copy_from_slice(&HFS_X_SIGNATURE_AND_VERSION);
	fs::write(&path, wrap(&trailing, Profile::Udro)).unwrap();
	assert!(matches!(
		open(&path),
		Err(ImageSourceError::NotWholeHfsPlus)
	));
}

#[test]
#[cfg(unix)]
fn refuses_symlink_input_and_detects_named_replacement() {
	let dir = tempfile::tempdir().unwrap();
	let parent = dir.path().join("images");
	fs::create_dir(&parent).unwrap();
	let path = parent.join("input.dmg");
	let alias = dir.path().join("alias.dmg");
	let bytes = wrap(&raw_disk(), Profile::Udro);
	fs::write(&path, &bytes).unwrap();
	std::os::unix::fs::symlink(&path, &alias).unwrap();
	assert!(matches!(open(&alias), Err(ImageSourceError::NotRegular)));
	let source = open(&path).unwrap();
	// Moving the containing directory leaves the held file's inode and
	// timestamps intact, isolating the named-entry check from the digest.
	fs::rename(&parent, dir.path().join("parked")).unwrap();
	fs::create_dir(&parent).unwrap();
	fs::write(&path, &bytes).unwrap();
	assert!(matches!(
		source.revalidate(),
		Err(ImageSourceError::PathChanged)
	));
}

#[test]
#[cfg(windows)]
fn denies_write_and_rename_sharing_while_source_is_held() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("input.dmg");
	let bytes = wrap(&raw_disk(), Profile::Udro);
	fs::write(&path, &bytes).unwrap();
	let source = open(&path).unwrap();
	assert!(fs::write(&path, b"changed").is_err());
	assert!(fs::rename(&path, dir.path().join("moved.dmg")).is_err());
	source.revalidate().unwrap();
	assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
#[cfg(target_os = "macos")]
fn admits_native_signed_bytes_without_treating_a_digest_check_as_trust() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("input.dmg");
	let raw = raw_disk();
	fs::write(&path, wrap(&raw, Profile::Udzo)).unwrap();
	let signed = std::process::Command::new("/usr/bin/codesign")
		.args(["--sign", "-"])
		.arg(&path)
		.output()
		.unwrap();
	assert!(signed.status.success(), "{signed:?}");
	let exact = fs::read(&path).unwrap();
	let expected = ImageRevision::from_sha256(Sha256::digest(&exact).into());
	let source = open(&path).unwrap();
	assert_eq!(source.revision(), expected);
	assert_eq!(source.envelope(), SourceEnvelope::SignedDigestsVerified);
	source.revalidate().unwrap();
	assert_eq!(fs::read(&path).unwrap(), exact);

	let mut changed = exact;
	let trailer = changed.last_chunk_mut::<TRAILER_BYTES>().unwrap();
	// Changing the segment identifier leaves CRC domains unchanged.
	const SEGMENT_ID_FIELD: usize = 64;
	trailer[SEGMENT_ID_FIELD] ^= 1;
	fs::write(&path, changed).unwrap();
	assert!(matches!(open(&path), Err(ImageSourceError::Signature(_))));
}
