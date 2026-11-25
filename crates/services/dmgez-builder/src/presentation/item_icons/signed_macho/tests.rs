use std::io::Write;

use super::*;

const LC_UUID: u32 = 0x1b;

fn probe(bytes: &[u8]) -> bool {
	let mut file = tempfile::tempfile().unwrap();
	file.write_all(bytes).unwrap();
	has_or_may_have_signature(&CapturedFile::new(file).unwrap()).unwrap()
}

fn thin(order: ByteOrder, wide: bool, signed: bool) -> Vec<u8> {
	let (magic, header_size) = match (order, wide) {
		(ByteOrder::Big, false) => (MH_MAGIC, MACH_HEADER_SIZE),
		(ByteOrder::Little, false) => (MH_CIGAM, MACH_HEADER_SIZE),
		(ByteOrder::Big, true) => (MH_MAGIC_64, MACH_HEADER_64_SIZE),
		(ByteOrder::Little, true) => (MH_CIGAM_64, MACH_HEADER_64_SIZE),
	};
	let encode = |value: u32| match order {
		ByteOrder::Big => value.to_be_bytes(),
		ByteOrder::Little => value.to_le_bytes(),
	};
	let mut bytes = vec![0; header_size as usize];
	bytes[..4].copy_from_slice(&magic.to_be_bytes());
	bytes[16..20].copy_from_slice(&encode(if signed { 2 } else { 1 }));
	bytes[20..24].copy_from_slice(&encode(if signed { 40 } else { 24 }));
	bytes.extend_from_slice(&encode(LC_UUID));
	bytes.extend_from_slice(&encode(24));
	bytes.extend_from_slice(&[0; 16]);
	if signed {
		bytes.extend_from_slice(&encode(LC_CODE_SIGNATURE));
		bytes.extend_from_slice(&encode(16));
		bytes.extend_from_slice(&[0; 8]);
	}
	bytes
}

fn universal(order: ByteOrder, wide: bool, slices: &[Vec<u8>]) -> Vec<u8> {
	let (magic, entry_size) = match (order, wide) {
		(ByteOrder::Big, false) => (FAT_MAGIC, FAT_ARCH_SIZE),
		(ByteOrder::Little, false) => (FAT_CIGAM, FAT_ARCH_SIZE),
		(ByteOrder::Big, true) => (FAT_MAGIC_64, FAT_ARCH_64_SIZE),
		(ByteOrder::Little, true) => (FAT_CIGAM_64, FAT_ARCH_64_SIZE),
	};
	let encode_u32 = |value: u32| match order {
		ByteOrder::Big => value.to_be_bytes(),
		ByteOrder::Little => value.to_le_bytes(),
	};
	let encode_u64 = |value: u64| match order {
		ByteOrder::Big => value.to_be_bytes(),
		ByteOrder::Little => value.to_le_bytes(),
	};
	let table_end = FAT_HEADER_SIZE as usize + slices.len() * entry_size as usize;
	let mut bytes = Vec::with_capacity(table_end);
	bytes.extend_from_slice(&magic.to_be_bytes());
	bytes.extend_from_slice(&encode_u32(slices.len() as u32));
	let mut start = table_end;
	for slice in slices {
		bytes.extend_from_slice(&[0; 8]);
		if wide {
			bytes.extend_from_slice(&encode_u64(start as u64));
			bytes.extend_from_slice(&encode_u64(slice.len() as u64));
			bytes.extend_from_slice(&[0; 8]);
		} else {
			bytes.extend_from_slice(&encode_u32(start as u32));
			bytes.extend_from_slice(&encode_u32(slice.len() as u32));
			bytes.extend_from_slice(&[0; 4]);
		}
		start += slice.len();
	}
	for slice in slices {
		bytes.extend_from_slice(slice);
	}
	bytes
}

#[test]
fn distinguishes_signed_and_unsigned_thin_mach_o_in_both_byte_orders() {
	for order in [ByteOrder::Big, ByteOrder::Little] {
		for wide in [false, true] {
			assert!(!probe(&thin(order, wide, false)));
			assert!(probe(&thin(order, wide, true)));
		}
	}
}

#[test]
fn inspects_every_slice_of_a_universal_mach_o() {
	for order in [ByteOrder::Big, ByteOrder::Little] {
		for wide in [false, true] {
			let unsigned = thin(ByteOrder::Big, false, false);
			let signed = thin(ByteOrder::Little, true, true);
			assert!(!probe(&universal(
				order,
				wide,
				&[unsigned.clone(), unsigned.clone()]
			)));
			assert!(probe(&universal(order, wide, &[unsigned, signed])));
		}
	}
}

#[test]
fn inspects_the_legacy_arm64_row_after_the_declared_table() {
	let unsigned = thin(ByteOrder::Little, true, false);
	for signed in [false, true] {
		let arm = thin(ByteOrder::Little, true, signed);
		let mut image = universal(ByteOrder::Big, false, &[unsigned.clone(), arm]);
		let count = FAT_ARCH_COUNT_OFFSET as usize;
		image[count..count + size_of::<u32>()].copy_from_slice(&1_u32.to_be_bytes());
		let extra = (FAT_HEADER_SIZE + FAT_ARCH_SIZE) as usize;
		image[extra..extra + size_of::<u32>()].copy_from_slice(&CPU_TYPE_ARM64.to_be_bytes());
		assert_eq!(probe(&image), signed);
	}
}

#[test]
fn protects_recognized_mach_o_with_incomplete_or_unsupported_tables() {
	assert!(probe(&MH_MAGIC.to_be_bytes()));
	assert!(probe(&FAT_MAGIC_64.to_be_bytes()));

	let mut command_overrun = thin(ByteOrder::Big, true, false);
	command_overrun[36..40].copy_from_slice(&32_u32.to_be_bytes());
	assert!(probe(&command_overrun));

	let mut inspection_limit = thin(ByteOrder::Big, false, false);
	let command_count = MAX_LOAD_COMMANDS + 1;
	let command_bytes = command_count * LOAD_COMMAND_HEADER_SIZE;
	inspection_limit[16..20].copy_from_slice(&command_count.to_be_bytes());
	inspection_limit[20..24].copy_from_slice(&command_bytes.to_be_bytes());
	inspection_limit.resize(MACH_HEADER_SIZE as usize + command_bytes as usize, 0);
	assert!(probe(&inspection_limit));

	let mut offset_overflow = universal(ByteOrder::Big, true, &[vec![0; 4]]);
	offset_overflow[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
	assert!(probe(&offset_overflow));
	assert!(probe(&universal(ByteOrder::Big, false, &[vec![0; 4]])));
}

#[test]
fn permits_java_class_files_that_share_the_universal_magic() {
	// Class A extends Object. The second variant's unused constant makes
	// the file long enough to resemble a universal architecture table.
	for extra_string_length in [0_u16, 1500] {
		let mut class = vec![
			0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61, 0, 5, 7, 0, 2, 1, 0, 1, b'A', 7, 0, 4, 1, 0, 16,
		];
		class.extend_from_slice(b"java/lang/Object");
		if extra_string_length > 0 {
			class[9] += 1;
			class.push(1);
			class.extend_from_slice(&extra_string_length.to_be_bytes());
			class.resize(class.len() + usize::from(extra_string_length), b'a');
		}
		class.extend_from_slice(&[0, 0x21, 0, 1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
		assert!(!probe(&class));
	}
	assert!(!probe(b"an ordinary data file"));
}
