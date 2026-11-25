use std::{
	fs,
	io::{Read, Seek, SeekFrom, Write},
	path::Path,
};

use super::fixture::NATIVE_BLOCK_BYTES;

pub(super) const INCOMPLETE_CHECKPOINT_XID: u64 = 5;

pub(super) fn advertise_incomplete_checkpoint(raw: &Path) {
	const BOOTSTRAP_BLOCK: u64 = 0;
	const INCOMPLETE_DESCRIPTOR_BLOCK: u64 = 6;
	const HEADER_XID_OFFSET: usize = 2 * size_of::<u64>();
	const NXSB_NEXT_XID_OFFSET: usize = 96;
	const NATIVE_XID: u64 = 4;
	const EARLIER_DESCRIPTOR_XID: u64 = 3;
	const NEXT_XID: u64 = INCOMPLETE_CHECKPOINT_XID + 1;
	edit_object(raw, BOOTSTRAP_BLOCK, |block| {
		replace_field(
			block,
			HEADER_XID_OFFSET,
			NATIVE_XID.to_le_bytes(),
			INCOMPLETE_CHECKPOINT_XID.to_le_bytes(),
		);
		replace_field(
			block,
			NXSB_NEXT_XID_OFFSET,
			INCOMPLETE_CHECKPOINT_XID.to_le_bytes(),
			NEXT_XID.to_le_bytes(),
		);
	});
	// The descriptor advertises XID 5 with a valid checksum, but the adjacent
	// mapping block still belongs to XID 3. The retained Apple XID 4 is complete.
	edit_object(raw, INCOMPLETE_DESCRIPTOR_BLOCK, |block| {
		replace_field(
			block,
			HEADER_XID_OFFSET,
			EARLIER_DESCRIPTOR_XID.to_le_bytes(),
			INCOMPLETE_CHECKPOINT_XID.to_le_bytes(),
		);
	});
}

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
