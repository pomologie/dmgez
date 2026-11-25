use std::io::{self, Read, Seek, SeekFrom};

use super::{
	ByteOrder, CPU_TYPE_ARM64, Classification, Layout, MACH_HEADER_64_SIZE, MAX_LOAD_COMMANDS,
	Region, ThinEncoding, classify_thin, layout, slice_region,
};

const MACH_HEADER_CPU_OFFSET: u64 = 4;
const CPU_TYPE_X86: u32 = 7;
const CPU_ARCH_ABI64: u32 = 0x0100_0000;
const CPU_TYPE_X86_64: u32 = CPU_TYPE_X86 | CPU_ARCH_ABI64;
const CPU_SUBTYPE_CAPABILITY_MASK: u32 = 0xff00_0000;
const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
const CPU_SUBTYPE_ARM64E: u32 = 2;
const CPU_SUBTYPE_X86_ALL: u32 = 3;
const CPU_SUBTYPE_X86_64_H: u32 = 8;

/// A Mach-O architecture with a native name suitable for slice selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Architecture {
	Arm64,
	Arm64e,
	I386,
	X86_64,
	X86_64h,
}

impl Architecture {
	/// Returns the native architecture selector.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Arm64 => "arm64",
			Self::Arm64e => "arm64e",
			Self::I386 => "i386",
			Self::X86_64 => "x86_64",
			Self::X86_64h => "x86_64h",
		}
	}

	fn from_cpu(cpu: Cpu) -> Option<Self> {
		match (cpu.kind, cpu.subtype) {
			(CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_ALL) => Some(Self::Arm64),
			(CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64E) => Some(Self::Arm64e),
			(CPU_TYPE_X86, CPU_SUBTYPE_X86_ALL) => Some(Self::I386),
			(CPU_TYPE_X86_64, CPU_SUBTYPE_X86_ALL) => Some(Self::X86_64),
			(CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_H) => Some(Self::X86_64h),
			_ => None,
		}
	}
}

/// Lists the addressable architectures of a signable Mach-O file in file order.
///
/// Returns `None` for nonsignable input, uncertain layout, mixed code kinds,
/// unsupported architectures, repeated selectors or disagreement between the
/// universal table and its slices. The stream position afterward is unspecified.
///
/// # Errors
///
/// Propagates I/O errors from the supplied reader.
pub fn architectures(reader: &mut (impl Read + Seek)) -> io::Result<Option<Vec<Architecture>>> {
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
		Layout::Thin(encoding) => Ok(describe(image, encoding, &mut read_at, &mut commands_left)?
			.map(|item| vec![item.architecture])),
		Layout::Fat {
			order,
			slices,
			java_collision: false,
		} => {
			let mut architectures = Vec::with_capacity(slices.len());
			let mut kind = None;
			for slice in slices {
				let region = slice_region(slice.range);
				let Some(encoding) = ThinEncoding::from_magic(region.magic(&mut read_at)?) else {
					return Ok(None);
				};
				let Some(item) = describe(region, encoding, &mut read_at, &mut commands_left)?
				else {
					return Ok(None);
				};
				if read_cpu(image, order, slice.entry_offset, &mut read_at)? != Some(item.cpu)
					|| kind.is_some_and(|prior| prior != item.kind)
					|| architectures.contains(&item.architecture)
				{
					return Ok(None);
				}
				kind = Some(item.kind);
				architectures.push(item.architecture);
			}
			Ok(Some(architectures))
		}
		_ => Ok(None),
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cpu {
	kind: u32,
	subtype: u32,
}

struct Slice {
	kind: super::CodeKind,
	cpu: Cpu,
	architecture: Architecture,
}

fn describe<E>(
	image: Region,
	encoding: ThinEncoding,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
	commands_left: &mut u32,
) -> Result<Option<Slice>, E> {
	let Classification::Code(kind) = classify_thin(image, encoding, read_at, commands_left)? else {
		return Ok(None);
	};
	let Some(cpu) = read_cpu(image, encoding.order, MACH_HEADER_CPU_OFFSET, read_at)? else {
		return Ok(None);
	};
	let wide_cpu = cpu.kind & CPU_ARCH_ABI64 != 0;
	if wide_cpu != (encoding.header_size == MACH_HEADER_64_SIZE) {
		return Ok(None);
	}
	Ok(Architecture::from_cpu(cpu).map(|architecture| Slice {
		kind,
		cpu,
		architecture,
	}))
}

fn read_cpu<E>(
	image: Region,
	order: ByteOrder,
	offset: u64,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<Option<Cpu>, E> {
	Ok(image
		.read(offset, read_at)?
		.map(|[a, b, c, d, e, f, g, h]| Cpu {
			kind: order.u32([a, b, c, d]),
			subtype: order.u32([e, f, g, h]) & !CPU_SUBTYPE_CAPABILITY_MASK,
		}))
}
