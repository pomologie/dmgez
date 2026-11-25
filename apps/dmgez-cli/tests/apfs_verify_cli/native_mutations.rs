use std::{
	fs,
	io::{Read, Seek, SeekFrom, Write},
	path::Path,
};

use super::fixture::NATIVE_BLOCK_BYTES;

pub(super) fn edit_object(
	raw: &Path,
	address: u64,
	edit: impl FnOnce(&mut [u8; NATIVE_BLOCK_BYTES]),
) {
	let mut file = fs::OpenOptions::new()
		.read(true)
		.write(true)
		.open(raw)
		.unwrap();
	let start = address * NATIVE_BLOCK_BYTES as u64;
	let mut block = [0; NATIVE_BLOCK_BYTES];
	file.seek(SeekFrom::Start(start)).unwrap();
	file.read_exact(&mut block).unwrap();
	assert_eq!(block[..size_of::<u64>()], fletcher64(&block).to_le_bytes());
	edit(&mut block);
	let checksum = fletcher64(&block);
	block[..size_of::<u64>()].copy_from_slice(&checksum.to_le_bytes());
	file.seek(SeekFrom::Start(start)).unwrap();
	file.write_all(&block).unwrap();
}

pub(super) fn replace_field<const N: usize>(
	block: &mut [u8; NATIVE_BLOCK_BYTES],
	offset: usize,
	original: [u8; N],
	replacement: [u8; N],
) {
	let field = &mut block[offset..][..N];
	assert_eq!(*field, original);
	field.copy_from_slice(&replacement);
}

// Correct the enclosing APFS object checksum so the checker reaches the
// intentionally damaged record. The original native checksum is a witness.
fn fletcher64(block: &[u8; NATIVE_BLOCK_BYTES]) -> u64 {
	const MODULUS: u64 = u32::MAX as u64;
	let mut first = 0;
	let mut second = 0;
	let (words, tail) = block[size_of::<u64>()..].as_chunks::<{ size_of::<u32>() }>();
	assert!(tail.is_empty());
	for word in words {
		first = (first + u64::from(u32::from_le_bytes(*word))) % MODULUS;
		second = (second + first) % MODULUS;
	}
	let low = MODULUS - (first + second) % MODULUS;
	let high = MODULUS - (first + low) % MODULUS;
	(high << u32::BITS) | low
}
