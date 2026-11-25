use std::{
	collections::BTreeSet,
	env, fs,
	io::{Seek, SeekFrom, Write},
	path::{Path, PathBuf},
};

const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_APFS_FIXTURES";
const PACKED_ADDRESS_BYTES: usize = size_of::<u64>();
const PACKED_RECORD_BYTES: usize = PACKED_ADDRESS_BYTES + NATIVE_BLOCK_BYTES;

pub(super) const NATIVE_BLOCK_BYTES: usize = 4096;
pub(super) const NATIVE_VOLUME_BYTES: u64 = 64 << 20;

// sonoma-hard-link-blocks.json records the independent Apple oracles.
pub(super) const HARD_LINK_FIXTURE: &str = "sonoma-hard-link-blocks.bin";
pub(super) const HARD_LINK_VOLUME_UUID: &str = "D37600DC-3EF5-4393-8E07-31DBAA91E9B6";
pub(super) const HARD_LINK_VOLUME_OID: &str = "1026";
pub(super) const HARD_LINK_CHECKPOINT_XID: &str = "4";
pub(super) const SPARSE_FIXTURE: &str = "sonoma-sparse-blocks.bin";

pub(super) fn materialize(image: &Path, fixture: &str, prefix: &[u8]) {
	let directory = env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies APFS fixtures");
	let path = PathBuf::from(directory).join(fixture);
	let packed = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
	let (records, remaining) = packed.as_chunks::<PACKED_RECORD_BYTES>();
	assert!(remaining.is_empty());

	let mut file = fs::File::create_new(image).unwrap();
	file.set_len(prefix.len() as u64 + NATIVE_VOLUME_BYTES)
		.unwrap();
	file.write_all(prefix).unwrap();
	let mut blocks = BTreeSet::new();
	for record in records {
		let (address, data) = record.split_first_chunk().unwrap();
		let block = u64::from_le_bytes(*address);
		assert!(blocks.insert(block));
		assert!(block < NATIVE_VOLUME_BYTES / NATIVE_BLOCK_BYTES as u64);
		let offset = prefix.len() as u64 + block * NATIVE_BLOCK_BYTES as u64;
		file.seek(SeekFrom::Start(offset)).unwrap();
		file.write_all(data).unwrap();
	}
}
