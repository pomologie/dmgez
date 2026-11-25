use std::{env, fs, path::PathBuf};

const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";

pub(super) fn hfsplus_fixture(name: &str) -> Vec<u8> {
	let root = env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies HFS+ fixtures");
	let path = PathBuf::from(root).join(name);
	fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// Sibling fixture ledgers describe the sparse format and the independent
// filesystem oracles. The CLI receives reconstructed raw disk bytes.
pub(super) fn materialize_sparse(mut fixture: &[u8], expected_length: usize) -> Vec<u8> {
	fn read_field<const WIDTH: usize>(input: &mut &[u8]) -> [u8; WIDTH] {
		let (value, rest) = input.split_first_chunk().unwrap();
		*input = rest;
		*value
	}

	let len = usize::try_from(u64::from_be_bytes(read_field(&mut fixture))).unwrap();
	assert_eq!(len, expected_length);
	let runs = u16::from_be_bytes(read_field(&mut fixture));
	let mut bytes = vec![0; len];
	for _ in 0..runs {
		let start = usize::try_from(u64::from_be_bytes(read_field(&mut fixture))).unwrap();
		let count = usize::try_from(u32::from_be_bytes(read_field(&mut fixture))).unwrap();
		let (record, rest) = fixture.split_at(count);
		bytes[start..][..count].copy_from_slice(record);
		fixture = rest;
	}
	assert!(fixture.is_empty());
	bytes
}
