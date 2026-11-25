use std::{
	io::{self, Read, Seek, SeekFrom},
	ops::Range,
};

mod architectures;
mod java;

pub use architectures::{Architecture, architectures};

use java::{JAVA_FIRST_MAJOR_VERSION, is_java_class_prefix};

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

const MACH_HEADER_FILE_TYPE_OFFSET: u64 = 12;
const MH_OBJECT: u32 = 0x1;
const MH_EXECUTE: u32 = 0x2;
const MH_CORE: u32 = 0x4;
const MH_DYLIB: u32 = 0x6;
const MH_BUNDLE: u32 = 0x8;
const MH_DYLIB_STUB: u32 = 0x9;
const MH_DSYM: u32 = 0xa;

/// Supported kinds of code that can be signed independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodeKind {
	Executable,
	Library,
}

/// Result of inspecting Mach-O headers and slice boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Classification {
	/// The input is ordinary data or a recognized nonsignable Mach-O type.
	Other,
	/// Recognized Mach-O could not be classified within the supported profile.
	Uncertain,
	/// All admitted Mach-O slices agree on the kind of signable code.
	Code(CodeKind),
}

#[derive(Clone, Copy)]
struct Region {
	start: u64,
	len: u64,
}

impl Region {
	fn read<const N: usize, E>(
		self,
		offset: u64,
		read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	) -> Result<Option<[u8; N]>, E> {
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
		read_at(offset, &mut bytes)?;
		Ok(Some(bytes))
	}

	fn magic<E>(
		self,
		read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	) -> Result<Option<u32>, E> {
		Ok(self.read(0, read_at)?.map(u32::from_be_bytes))
	}
}

#[derive(Clone, Copy)]
struct ThinEncoding {
	order: ByteOrder,
	header_size: u64,
	command_alignment: u32,
}

impl ThinEncoding {
	fn from_magic(magic: Option<u32>) -> Option<Self> {
		let (order, wide) = match magic {
			Some(MH_MAGIC) => (ByteOrder::Big, false),
			Some(MH_CIGAM) => (ByteOrder::Little, false),
			Some(MH_MAGIC_64) => (ByteOrder::Big, true),
			Some(MH_CIGAM_64) => (ByteOrder::Little, true),
			_ => return None,
		};
		let (header_size, command_alignment) = if wide {
			(MACH_HEADER_64_SIZE, MACH_COMMAND_64_ALIGNMENT)
		} else {
			(MACH_HEADER_SIZE, MACH_COMMAND_ALIGNMENT)
		};
		Some(Self {
			order,
			header_size,
			command_alignment,
		})
	}

	fn command_table<E>(
		self,
		image: Region,
		read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
		commands_left: &mut u32,
	) -> Result<Option<CommandTable>, E> {
		let Some([a, b, c, d, e, f, g, h]) = image.read(MACH_HEADER_COMMANDS_OFFSET, read_at)?
		else {
			return Ok(None);
		};
		let count = self.order.u32([a, b, c, d]);
		let bytes = self.order.u32([e, f, g, h]);
		let end = self.header_size + u64::from(bytes);
		if end > image.len || count > bytes / LOAD_COMMAND_HEADER_SIZE {
			return Ok(None);
		}
		let Some(remaining) = commands_left.checked_sub(count) else {
			return Ok(None);
		};
		*commands_left = remaining;
		Ok(Some(CommandTable { count, end }))
	}
}

struct CommandTable {
	count: u32,
	end: u64,
}

enum Layout {
	Other,
	Uncertain,
	Thin(ThinEncoding),
	Fat {
		order: ByteOrder,
		slices: Vec<FatSlice>,
		java_collision: bool,
	},
}

struct FatSlice {
	entry_offset: u64,
	range: Range<u64>,
}

/// Determines whether a thin or universal file contains executable or library code.
///
/// This is a structural classification of bounded headers and slices. The
/// reader's offset zero must correspond to the image start. The stream position after inspection is unspecified.
///
/// # Errors
///
/// Propagates I/O errors from the supplied reader.
pub fn classify(reader: &mut (impl Read + Seek)) -> io::Result<Classification> {
	let image = Region {
		start: 0,
		len: reader.seek(SeekFrom::End(0))?,
	};
	let mut read_at = |offset, output: &mut [u8]| {
		reader.seek(SeekFrom::Start(offset))?;
		reader.read_exact(output)
	};
	let mut commands_left = MAX_LOAD_COMMANDS;
	match layout(image, &mut read_at)? {
		Layout::Other => Ok(Classification::Other),
		Layout::Uncertain => Ok(Classification::Uncertain),
		Layout::Thin(encoding) => classify_thin(image, encoding, &mut read_at, &mut commands_left),
		Layout::Fat {
			slices,
			java_collision,
			..
		} => {
			let mut result = None;
			for slice in slices {
				let region = slice_region(slice.range);
				let Some(encoding) = ThinEncoding::from_magic(region.magic(&mut read_at)?) else {
					if java_collision {
						continue;
					}
					return Ok(Classification::Uncertain);
				};
				if java_collision {
					return Ok(Classification::Uncertain);
				}
				let current = classify_thin(region, encoding, &mut read_at, &mut commands_left)?;
				if current == Classification::Uncertain
					|| result.is_some_and(|prior| prior != current)
				{
					return Ok(Classification::Uncertain);
				}
				result = Some(current);
			}
			Ok(result.unwrap_or(Classification::Other))
		}
	}
}

/// Detects a signature load command or recognized Mach-O that cannot be ruled out.
///
/// The callback must fill the requested byte range at an absolute offset or
/// return an error. Requests remain within the supplied file length. This is
/// a conservative mutation guard and does not validate any signature.
///
/// # Errors
///
/// Propagates errors from the positional reader unchanged.
pub fn has_or_may_have_signature_at<E>(
	len: u64,
	mut read_at: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<bool, E> {
	let image = Region { start: 0, len };
	let mut commands_left = MAX_LOAD_COMMANDS;
	match layout(image, &mut read_at)? {
		Layout::Other => Ok(false),
		Layout::Uncertain => Ok(true),
		Layout::Thin(encoding) => {
			thin_has_signature(image, encoding, &mut read_at, &mut commands_left)
		}
		Layout::Fat {
			slices,
			java_collision,
			..
		} => {
			for slice in slices {
				let region = slice_region(slice.range);
				if let Some(encoding) = ThinEncoding::from_magic(region.magic(&mut read_at)?) {
					if thin_has_signature(region, encoding, &mut read_at, &mut commands_left)? {
						return Ok(true);
					}
				} else if !java_collision {
					return Ok(true);
				}
			}
			Ok(false)
		}
	}
}

fn classify_thin<E>(
	image: Region,
	encoding: ThinEncoding,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	commands_left: &mut u32,
) -> Result<Classification, E> {
	if encoding
		.command_table(image, read_at, commands_left)?
		.is_none()
	{
		return Ok(Classification::Uncertain);
	}
	let Some(file_type) = image.read(MACH_HEADER_FILE_TYPE_OFFSET, read_at)? else {
		return Ok(Classification::Uncertain);
	};
	Ok(match encoding.order.u32(file_type) {
		MH_EXECUTE => Classification::Code(CodeKind::Executable),
		MH_DYLIB | MH_BUNDLE => Classification::Code(CodeKind::Library),
		MH_OBJECT | MH_CORE | MH_DYLIB_STUB | MH_DSYM => Classification::Other,
		_ => Classification::Uncertain,
	})
}

fn thin_has_signature<E>(
	image: Region,
	encoding: ThinEncoding,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	commands_left: &mut u32,
) -> Result<bool, E> {
	let Some(CommandTable { count, end }) =
		encoding.command_table(image, read_at, commands_left)?
	else {
		return Ok(true);
	};
	let mut cursor = encoding.header_size;
	for _ in 0..count {
		if end - cursor < u64::from(LOAD_COMMAND_HEADER_SIZE) {
			return Ok(true);
		}
		let Some([a, b, c, d, e, f, g, h]) = image.read(cursor, read_at)? else {
			return Ok(true);
		};
		if encoding.order.u32([a, b, c, d]) == LC_CODE_SIGNATURE {
			return Ok(true);
		}
		let size = encoding.order.u32([e, f, g, h]);
		if size < LOAD_COMMAND_HEADER_SIZE
			|| !size.is_multiple_of(encoding.command_alignment)
			|| u64::from(size) > end - cursor
		{
			return Ok(true);
		}
		cursor += u64::from(size);
	}
	Ok(cursor != end)
}

fn layout<E>(
	image: Region,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Layout, E> {
	let (order, wide) = match image.magic(read_at)? {
		Some(FAT_MAGIC) => (ByteOrder::Big, false),
		Some(FAT_CIGAM) => (ByteOrder::Little, false),
		Some(FAT_MAGIC_64) => (ByteOrder::Big, true),
		Some(FAT_CIGAM_64) => (ByteOrder::Little, true),
		magic => return Ok(ThinEncoding::from_magic(magic).map_or(Layout::Other, Layout::Thin)),
	};
	fat_layout(image, order, wide, read_at)
}

fn fat_layout<E>(
	image: Region,
	order: ByteOrder,
	wide: bool,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Layout, E> {
	let Some(count_bytes) = image.read(FAT_ARCH_COUNT_OFFSET, read_at)? else {
		return Ok(Layout::Uncertain);
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
		&& is_java_class_prefix(image, read_at)?;
	if count == 0 || table_end > image.len {
		return Ok(if java_class {
			Layout::Other
		} else {
			Layout::Uncertain
		});
	}
	if count > MAX_FAT_ARCHITECTURES {
		return Ok(Layout::Uncertain);
	}

	// Apple's signature reader recognizes an ARM64 row after the declared
	// classic table, even when the header's architecture count excludes it.
	let extra_arm64 = !wide
		&& image.read(table_end, read_at)?.map(|word| order.u32(word)) == Some(CPU_TYPE_ARM64);
	let count = count + u32::from(extra_arm64);
	let table_end = FAT_HEADER_SIZE + u64::from(count) * arch_size;
	if table_end > image.len {
		return Ok(Layout::Uncertain);
	}
	let mut slices = Vec::with_capacity(count as usize);
	let mut invalid_table = false;
	for index in 0..count {
		let entry_offset = FAT_HEADER_SIZE + u64::from(index) * arch_size;
		let Some((start, len)) = fat_arch_range(image, order, wide, entry_offset, read_at)? else {
			return Ok(Layout::Uncertain);
		};
		let Some(end) = start.checked_add(len) else {
			return Ok(Layout::Uncertain);
		};
		// The classic universal format cannot have slices inside its table,
		// beyond the file, or whose end exceeds the 32-bit offset range.
		if start < table_end || end > image.len || (!wide && end > u64::from(u32::MAX)) {
			if !java_class || extra_arm64 {
				return Ok(Layout::Uncertain);
			}
			invalid_table = true;
			continue;
		}
		slices.push(FatSlice {
			entry_offset,
			range: start..end,
		});
	}
	slices.sort_unstable_by_key(|slice| slice.range.start);
	invalid_table |= slices
		.windows(2)
		.any(|pair| pair[0].range.end > pair[1].range.start);
	let java_collision = java_class && !extra_arm64 && invalid_table;
	if invalid_table && !java_collision {
		return Ok(Layout::Uncertain);
	}
	Ok(Layout::Fat {
		order,
		slices,
		java_collision,
	})
}

fn slice_region(slice: Range<u64>) -> Region {
	Region {
		start: slice.start,
		len: slice.end - slice.start,
	}
}

fn fat_arch_range<E>(
	image: Region,
	order: ByteOrder,
	wide: bool,
	entry_offset: u64,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Option<(u64, u64)>, E> {
	const FAT_ARCH_OFFSET_FIELD: u64 = 8;

	let offset = entry_offset + FAT_ARCH_OFFSET_FIELD;
	if wide {
		let Some([a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p]) = image.read(offset, read_at)?
		else {
			return Ok(None);
		};
		return Ok(Some((
			order.u64([a, b, c, d, e, f, g, h]),
			order.u64([i, j, k, l, m, n, o, p]),
		)));
	}
	let Some([a, b, c, d, e, f, g, h]) = image.read(offset, read_at)? else {
		return Ok(None);
	};
	Ok(Some((
		u64::from(order.u32([a, b, c, d])),
		u64::from(order.u32([e, f, g, h])),
	)))
}
#[cfg(test)]
mod tests;
