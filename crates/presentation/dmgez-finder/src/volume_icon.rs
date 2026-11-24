//! Admission for a prebuilt ICNS asset and its root catalog Finder flag.

use std::{collections::BTreeSet, io::Cursor};

use icns::{IconElement, IconType, OSType};
use plist::stream::{BinaryReader, Event};
use thiserror::Error;

/// The reserved asset filename in the root catalog.
pub const NAME: &str = ".VolumeIcon.icns";
/// Maximum encoded source size for the qualified prebuilt profile.
pub const MAX_BYTES: usize = 16 << 20;

type Kind = [u8; FOURCC_BYTES];

const FOURCC_BYTES: usize = 4;
const LENGTH_BYTES: usize = 4;
const HEADER_BYTES: usize = FOURCC_BYTES + LENGTH_BYTES;
const ICNS_MAGIC: Kind = *b"icns";
const INFO_ELEMENT: Kind = *b"info";
const ARGB_MAGIC: Kind = *b"ARGB";
const MAX_INFO_BYTES: usize = 4 << 10;
const MAX_INFO_EVENTS: usize = 256;
const PNG_MAGIC: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
const PNG_IHDR: Kind = *b"IHDR";
const PNG_IEND: Kind = *b"IEND";
const PNG_ANIMATION_CHUNKS: [Kind; 3] = [*b"acTL", *b"fcTL", *b"fdAT"];
const PNG_CHUNK_CRC_BYTES: usize = 4;
const PNG_IHDR_BYTES: usize = 13;
const MAX_PNG_CHUNKS: usize = 4096;
const FINDER_INFO_BYTES: usize = 32;
const ROOT_FINDER_FLAGS: std::ops::Range<usize> = 8..10;
const HAS_CUSTOM_ICON: u16 = 0x0400;

/// A structurally admitted ICNS asset retaining the original encoded bytes.
#[derive(Clone, Copy)]
pub struct VolumeIcon<'source>(&'source [u8]);

impl std::fmt::Debug for VolumeIcon<'_> {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter
			.debug_struct("VolumeIcon")
			.field("bytes", &self.0.len())
			.finish()
	}
}

impl<'source> VolumeIcon<'source> {
	/// Returns the asset as it was supplied, including its original element order.
	#[must_use]
	pub const fn as_bytes(self) -> &'source [u8] {
		self.0
	}

	/// Sets the custom-icon flag in the root folder's catalog Finder information.
	///
	/// The HFS+ volume header carries a separate field for blessed folder IDs
	/// and the volume identifier.
	pub fn mark_root_finder_info(self, finder_info: &mut [u8; FINDER_INFO_BYTES]) {
		let flags = &mut finder_info[ROOT_FINDER_FLAGS];
		let current = u16::from_be_bytes([flags[0], flags[1]]);
		flags.copy_from_slice(&(current | HAS_CUSTOM_ICON).to_be_bytes());
	}
}

/// A volume icon could not enter the prebuilt profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("ICNS source exceeds the {MAX_BYTES}-byte limit")]
	Size,
	#[error("invalid ICNS file header")]
	FileHeader,
	#[error("ICNS declared length differs from source length")]
	FileLength,
	#[error("invalid ICNS element length at byte {offset}")]
	ElementLength { offset: usize },
	#[error("duplicate ICNS element {kind:02x?}")]
	DuplicateElement { kind: Kind },
	#[error("unsupported ICNS element {kind:02x?}")]
	UnsupportedElement { kind: Kind },
	#[error("unsupported encoding for ICNS element {kind:02x?}")]
	UnsupportedEncoding { kind: Kind },
	#[error("invalid PNG framing in ICNS element {kind:02x?}")]
	PngFraming { kind: Kind },
	#[error("PNG dimensions do not match ICNS element {kind:02x?}")]
	PngDimensions { kind: Kind },
	#[error("invalid PNG chunk checksum in ICNS element {kind:02x?}")]
	PngChecksum { kind: Kind },
	#[error("could not decode ICNS element {kind:02x?}")]
	Decode {
		kind: Kind,
		#[source]
		source: std::io::Error,
	},
	#[error("invalid or over-budget ICNS information element")]
	Info,
	#[error("ICNS contains no image representations")]
	NoImage,
}

/// Validates ICNS framing and decodes the supported image representations.
///
/// The modern profile accepts nonanimated PNG images up to 1024×1024 pixels
/// with fixed dimensions and native small planar ARGB representations. It
/// checks PNG framing, dimensions and CRCs before decoding. The original
/// encoded asset remains available via [VolumeIcon::as_bytes].
///
/// # Errors
///
/// Rejects malformed data, unsupported encodings, duplicate element types,
/// and sources outside the profile's size or validation budgets.
pub fn validate(bytes: &[u8]) -> Result<VolumeIcon<'_>, Error> {
	if bytes.len() > MAX_BYTES {
		return Err(Error::Size);
	}
	let (magic, declared_len) = kind_and_length(bytes).ok_or(Error::FileHeader)?;
	if magic != ICNS_MAGIC {
		return Err(Error::FileHeader);
	}
	if declared_len != bytes.len() {
		return Err(Error::FileLength);
	}
	let mut seen = BTreeSet::new();
	let mut has_image = false;
	let mut elements = &bytes[HEADER_BYTES..];
	while !elements.is_empty() {
		let length_error = || Error::ElementLength {
			offset: bytes.len() - elements.len(),
		};
		let (kind, element_len) = kind_and_length(elements).ok_or_else(length_error)?;
		let (element, rest) = elements
			.split_at_checked(element_len)
			.ok_or_else(length_error)?;
		let payload = element.get(HEADER_BYTES..).ok_or_else(length_error)?;
		if !seen.insert(kind) {
			return Err(Error::DuplicateElement { kind });
		}
		if kind == INFO_ELEMENT {
			validate_info(payload)?;
		} else {
			validate_image(kind, payload)?;
			has_image = true;
		}
		elements = rest;
	}
	if !has_image {
		return Err(Error::NoImage);
	}
	Ok(VolumeIcon(bytes))
}

fn kind_and_length(bytes: &[u8]) -> Option<(Kind, usize)> {
	let (kind, rest) = bytes.split_first_chunk()?;
	let (size, _) = rest.split_first_chunk()?;
	Some((*kind, usize::try_from(u32::from_be_bytes(*size)).ok()?))
}

fn validate_info(bytes: &[u8]) -> Result<(), Error> {
	if bytes.len() > MAX_INFO_BYTES {
		return Err(Error::Info);
	}
	// The lazy plist stream bounds work even if object references repeat.
	let events = BinaryReader::new(Cursor::new(bytes));
	let mut events = events.take(MAX_INFO_EVENTS + 1).enumerate();
	if !matches!(events.next(), Some((0, Ok(Event::StartDictionary(_))))) {
		return Err(Error::Info);
	}
	for (index, event) in events {
		if index == MAX_INFO_EVENTS || event.is_err() {
			return Err(Error::Info);
		}
	}
	Ok(())
}

fn validate_image(kind: Kind, bytes: &[u8]) -> Result<(), Error> {
	use IconType::{
		RGBA32_16x16, RGBA32_16x16_2x, RGBA32_16x16_2x_ARGB, RGBA32_16x16_ARGB, RGBA32_32x32,
		RGBA32_32x32_2x, RGBA32_64x64, RGBA32_128x128, RGBA32_128x128_2x, RGBA32_256x256,
		RGBA32_256x256_2x, RGBA32_512x512, RGBA32_512x512_2x,
	};

	let Some(icon_type) = IconType::from_ostype(OSType(kind)) else {
		return Err(Error::UnsupportedElement { kind });
	};
	let argb_allowed = match icon_type {
		RGBA32_16x16_ARGB | RGBA32_16x16_2x_ARGB => true,
		RGBA32_16x16 | RGBA32_16x16_2x | RGBA32_32x32 | RGBA32_32x32_2x | RGBA32_64x64
		| RGBA32_128x128 | RGBA32_128x128_2x | RGBA32_256x256 | RGBA32_256x256_2x
		| RGBA32_512x512 | RGBA32_512x512_2x => false,
		_ => return Err(Error::UnsupportedElement { kind }),
	};
	if let Some(chunks) = bytes.strip_prefix(&PNG_MAGIC) {
		validate_png(kind, icon_type, chunks)?;
	} else if !(argb_allowed && bytes.starts_with(&ARGB_MAGIC)) {
		return Err(Error::UnsupportedEncoding { kind });
	}
	let element = IconElement::new(OSType(kind), bytes.to_vec());
	element
		.decode_image()
		.map(|_| ())
		.map_err(|source| Error::Decode { kind, source })
}

fn validate_png(kind: Kind, icon_type: IconType, mut chunks: &[u8]) -> Result<(), Error> {
	for index in 0..MAX_PNG_CHUNKS {
		let chunk = png_chunk(chunks).ok_or(Error::PngFraming { kind })?;
		if index == 0 {
			if chunk.tag != PNG_IHDR || chunk.payload.len() != PNG_IHDR_BYTES {
				return Err(Error::PngFraming { kind });
			}
			let (width, rest) = chunk
				.payload
				.split_first_chunk()
				.ok_or(Error::PngFraming { kind })?;
			let (height, _) = rest.split_first_chunk().ok_or(Error::PngFraming { kind })?;
			if u32::from_be_bytes(*width) != icon_type.pixel_width()
				|| u32::from_be_bytes(*height) != icon_type.pixel_height()
			{
				return Err(Error::PngDimensions { kind });
			}
		}
		if PNG_ANIMATION_CHUNKS.contains(&chunk.tag) {
			return Err(Error::UnsupportedEncoding { kind });
		}
		let crc_end = HEADER_BYTES + chunk.payload.len();
		if crc32fast::hash(&chunks[LENGTH_BYTES..crc_end]) != chunk.expected_crc {
			return Err(Error::PngChecksum { kind });
		}
		if chunk.tag == PNG_IEND {
			return if chunk.payload.is_empty() && chunk.remainder.is_empty() {
				Ok(())
			} else {
				Err(Error::PngFraming { kind })
			};
		}
		chunks = chunk.remainder;
	}
	Err(Error::PngFraming { kind })
}

struct PngChunk<'source> {
	tag: Kind,
	payload: &'source [u8],
	expected_crc: u32,
	remainder: &'source [u8],
}

fn png_chunk(bytes: &[u8]) -> Option<PngChunk<'_>> {
	let (length, rest) = bytes.split_first_chunk()?;
	let length = usize::try_from(u32::from_be_bytes(*length)).ok()?;
	let (tag, rest) = rest.split_first_chunk()?;
	let (payload, rest) = rest.split_at_checked(length)?;
	let (crc, remainder) = rest.split_first_chunk::<PNG_CHUNK_CRC_BYTES>()?;
	Some(PngChunk {
		tag: *tag,
		payload,
		expected_crc: u32::from_be_bytes(*crc),
		remainder,
	})
}
