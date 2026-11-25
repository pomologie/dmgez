use std::ops::Range;

use dmgez_core::source::{CapturedFile, SourceError};

const MH_MAGIC: u32 = 0xfeed_face;
const MH_CIGAM: u32 = 0xcefa_edfe;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const FAT_CIGAM_64: u32 = 0xbfba_feca;
const LC_CODE_SIGNATURE: u32 = 0x1d;

const MACH_HEADER_SIZE: u64 = 28;
const MACH_HEADER_64_SIZE: u64 = 32;
const MACH_HEADER_COMMANDS_OFFSET: u64 = 16;
const LOAD_COMMAND_HEADER_SIZE: u32 = 8;
const MACH_COMMAND_ALIGNMENT: u32 = 4;
const MACH_COMMAND_64_ALIGNMENT: u32 = 8;
const FAT_HEADER_SIZE: u64 = 8;
const FAT_ARCH_COUNT_OFFSET: u64 = 4;
const FAT_ARCH_SIZE: u64 = 20;
const FAT_ARCH_64_SIZE: u64 = 32;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MAX_FAT_ARCHITECTURES: u32 = 256;
const MAX_LOAD_COMMANDS: u32 = 4096;

const JAVA_FIRST_MAJOR_VERSION: u16 = 45;
const JAVA_PREVIEW_FIRST_MAJOR_VERSION: u16 = 56;
const JAVA_PREVIEW_MINOR_VERSION: u16 = u16::MAX;
const JAVA_UTF8_TAG: u8 = 1;
const JAVA_INTEGER_TAG: u8 = 3;
const JAVA_NAME_AND_TYPE_TAG: u8 = 12;
const JAVA_METHOD_HANDLE_TAG: u8 = 15;
const JAVA_PACKAGE_TAG: u8 = 20;

#[derive(Clone, Copy)]
enum ByteOrder {
	Big,
	Little,
}

impl ByteOrder {
	fn u32(self, bytes: [u8; 4]) -> u32 {
		match self {
			Self::Big => u32::from_be_bytes(bytes),
			Self::Little => u32::from_le_bytes(bytes),
		}
	}

	fn u64(self, bytes: [u8; 8]) -> u64 {
		match self {
			Self::Big => u64::from_be_bytes(bytes),
			Self::Little => u64::from_le_bytes(bytes),
		}
	}
}

struct Image<'a> {
	source: &'a CapturedFile,
	start: u64,
	len: u64,
}

impl Image<'_> {
	fn read<const N: usize>(&self, offset: u64) -> Result<Option<[u8; N]>, SourceError> {
		if offset
			.checked_add(N as u64)
			.is_none_or(|end| end > self.len)
		{
			return Ok(None);
		}
		let Some(offset) = self.start.checked_add(offset) else {
			return Ok(None);
		};
		let mut bytes = [0; N];
		self.source.read_exact_at(offset, &mut bytes)?;
		Ok(Some(bytes))
	}

	fn magic(&self) -> Result<Option<u32>, SourceError> {
		Ok(self.read(0)?.map(u32::from_be_bytes))
	}
}

/// Returns true for a signature load command or a recognized Mach-O whose
/// command table cannot be inspected within the supported format and limits.
pub(super) fn has_or_may_have_signature(source: &CapturedFile) -> Result<bool, SourceError> {
	let image = Image {
		source,
		start: 0,
		len: source.len(),
	};
	let mut commands_left = MAX_LOAD_COMMANDS;
	match image.magic()? {
		Some(FAT_MAGIC) => fat_has_signature(&image, ByteOrder::Big, false, &mut commands_left),
		Some(FAT_CIGAM) => fat_has_signature(&image, ByteOrder::Little, false, &mut commands_left),
		Some(FAT_MAGIC_64) => fat_has_signature(&image, ByteOrder::Big, true, &mut commands_left),
		Some(FAT_CIGAM_64) => {
			fat_has_signature(&image, ByteOrder::Little, true, &mut commands_left)
		}
		magic => Ok(thin_has_signature(&image, magic, &mut commands_left)?.unwrap_or(false)),
	}
}

fn thin_has_signature(
	image: &Image<'_>,
	magic: Option<u32>,
	commands_left: &mut u32,
) -> Result<Option<bool>, SourceError> {
	let (order, header_size, alignment) = match magic {
		Some(MH_MAGIC) => (ByteOrder::Big, MACH_HEADER_SIZE, MACH_COMMAND_ALIGNMENT),
		Some(MH_CIGAM) => (ByteOrder::Little, MACH_HEADER_SIZE, MACH_COMMAND_ALIGNMENT),
		Some(MH_MAGIC_64) => (
			ByteOrder::Big,
			MACH_HEADER_64_SIZE,
			MACH_COMMAND_64_ALIGNMENT,
		),
		Some(MH_CIGAM_64) => (
			ByteOrder::Little,
			MACH_HEADER_64_SIZE,
			MACH_COMMAND_64_ALIGNMENT,
		),
		_ => return Ok(None),
	};
	let Some([a, b, c, d, e, f, g, h]) = image.read(MACH_HEADER_COMMANDS_OFFSET)? else {
		return Ok(Some(true));
	};
	let command_count = order.u32([a, b, c, d]);
	let command_bytes = order.u32([e, f, g, h]);
	let commands_end = header_size + u64::from(command_bytes);
	if commands_end > image.len || command_count > command_bytes / LOAD_COMMAND_HEADER_SIZE {
		return Ok(Some(true));
	}
	let Some(remaining_budget) = commands_left.checked_sub(command_count) else {
		return Ok(Some(true));
	};
	*commands_left = remaining_budget;

	let mut cursor = header_size;
	for _ in 0..command_count {
		if commands_end - cursor < u64::from(LOAD_COMMAND_HEADER_SIZE) {
			return Ok(Some(true));
		}
		let Some([a, b, c, d, e, f, g, h]) = image.read(cursor)? else {
			return Ok(Some(true));
		};
		if order.u32([a, b, c, d]) == LC_CODE_SIGNATURE {
			return Ok(Some(true));
		}
		let command_size = order.u32([e, f, g, h]);
		if command_size < LOAD_COMMAND_HEADER_SIZE
			|| !command_size.is_multiple_of(alignment)
			|| u64::from(command_size) > commands_end - cursor
		{
			return Ok(Some(true));
		}
		cursor += u64::from(command_size);
	}
	Ok(Some(cursor != commands_end))
}

fn fat_has_signature(
	image: &Image<'_>,
	order: ByteOrder,
	wide: bool,
	commands_left: &mut u32,
) -> Result<bool, SourceError> {
	let Some(count_bytes) = image.read(FAT_ARCH_COUNT_OFFSET)? else {
		return Ok(true);
	};
	let count = order.u32(count_bytes);
	let arch_size = if wide {
		FAT_ARCH_64_SIZE
	} else {
		FAT_ARCH_SIZE
	};
	let table_end = FAT_HEADER_SIZE + u64::from(count) * arch_size;
	let java_class = !wide
		&& matches!(order, ByteOrder::Big)
		&& count >= u32::from(JAVA_FIRST_MAJOR_VERSION)
		&& is_java_class_prefix(image)?;
	if count == 0 || table_end > image.len {
		return Ok(!java_class);
	}
	if count > MAX_FAT_ARCHITECTURES {
		return Ok(true);
	}

	// Apple's signature reader recognizes an ARM64 row after the declared
	// classic table, even when the header's architecture count excludes it.
	let extra_arm64 =
		!wide && image.read(table_end)?.map(|word| order.u32(word)) == Some(CPU_TYPE_ARM64);
	let count = count + u32::from(extra_arm64);
	let table_end = FAT_HEADER_SIZE + u64::from(count) * arch_size;
	if table_end > image.len {
		return Ok(true);
	}
	let mut ranges = Vec::with_capacity(count as usize);
	let mut invalid_table = false;
	for index in 0..count {
		let entry_offset = FAT_HEADER_SIZE + u64::from(index) * arch_size;
		let Some((start, len)) = fat_arch_range(image, order, wide, entry_offset)? else {
			return Ok(true);
		};
		let Some(end) = start.checked_add(len) else {
			return Ok(true);
		};
		// The classic universal format cannot have slices inside its table,
		// beyond the file, or whose end exceeds the 32-bit offset range.
		if start < table_end || end > image.len || (!wide && end > u64::from(u32::MAX)) {
			if !java_class || extra_arm64 {
				return Ok(true);
			}
			invalid_table = true;
			continue;
		}
		ranges.push(start..end);
	}

	ranges.sort_unstable_by_key(|range| range.start);
	invalid_table |= ranges.windows(2).any(|pair| pair[0].end > pair[1].start);
	let java_collision = java_class && !extra_arm64 && invalid_table;
	if invalid_table && !java_collision {
		return Ok(true);
	}
	for Range { start, end } in ranges {
		let slice = Image {
			source: image.source,
			start,
			len: end - start,
		};
		if thin_has_signature(&slice, slice.magic()?, commands_left)?.unwrap_or(!java_collision) {
			return Ok(true);
		}
	}
	Ok(false)
}

fn fat_arch_range(
	image: &Image<'_>,
	order: ByteOrder,
	wide: bool,
	entry_offset: u64,
) -> Result<Option<(u64, u64)>, SourceError> {
	const FAT_ARCH_OFFSET_FIELD: u64 = 8;

	let offset = entry_offset + FAT_ARCH_OFFSET_FIELD;
	if wide {
		let Some([a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p]) = image.read(offset)? else {
			return Ok(None);
		};
		return Ok(Some((
			order.u64([a, b, c, d, e, f, g, h]),
			order.u64([i, j, k, l, m, n, o, p]),
		)));
	}
	let Some([a, b, c, d, e, f, g, h]) = image.read(offset)? else {
		return Ok(None);
	};
	Ok(Some((
		u64::from(order.u32([a, b, c, d])),
		u64::from(order.u32([e, f, g, h])),
	)))
}

fn is_java_class_prefix(image: &Image<'_>) -> Result<bool, SourceError> {
	// Java ClassFile shares FAT_MAGIC. Only use the Java prefix when the
	// same bytes cannot describe the classic universal file's slice table.
	let Some(
		[
			_,
			_,
			_,
			_,
			minor_hi,
			minor_lo,
			major_hi,
			major_lo,
			pool_hi,
			pool_lo,
			first_tag,
		],
	) = image.read(0)?
	else {
		return Ok(false);
	};
	let minor = u16::from_be_bytes([minor_hi, minor_lo]);
	let major = u16::from_be_bytes([major_hi, major_lo]);
	let pool_count = u16::from_be_bytes([pool_hi, pool_lo]);
	Ok(major >= JAVA_FIRST_MAJOR_VERSION
		&& (major < JAVA_PREVIEW_FIRST_MAJOR_VERSION
			|| minor == 0
			|| minor == JAVA_PREVIEW_MINOR_VERSION)
		&& pool_count > 1
		// JVM ClassFile constant_pool tags have gaps for reserved values.
		&& matches!(
			first_tag,
			JAVA_UTF8_TAG | JAVA_INTEGER_TAG..=JAVA_NAME_AND_TYPE_TAG | JAVA_METHOD_HANDLE_TAG..=JAVA_PACKAGE_TAG
		))
}

#[cfg(test)]
mod tests;
