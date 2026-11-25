//! ICNS assets and the root catalog Finder flag.

use std::{collections::BTreeSet, io::Cursor};

use icns::{IconElement, IconType, OSType};
use plist::stream::{BinaryReader, Event};
use thiserror::Error;

use crate::finder_info::{self, FINDER_INFO_BYTES, HAS_CUSTOM_ICON};

#[cfg(feature = "badge-icons")]
mod badge;
mod encoding;

#[cfg(feature = "badge-icons")]
pub use badge::{MAX_BADGE_BYTES, badge_memory, encode_badge};
pub use encoding::{encode_pngs, png_encoding_memory, validation_memory};

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
		mark_root_finder_info(finder_info);
	}
}

/// Sets the custom-icon flag in the root folder's catalog Finder information.
///
/// Apply this after validating a prebuilt icon or generating one with [encode_pngs].
pub fn mark_root_finder_info(finder_info: &mut [u8; FINDER_INFO_BYTES]) {
	finder_info::add_flags(finder_info, HAS_CUSTOM_ICON);
}

/// A volume icon could not enter the supported profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("ICNS source exceeds the {MAX_BYTES}-byte limit")]
	Size,
	#[error("generated ICNS does not fit the output byte budget")]
	OutputBudget,
	#[cfg(feature = "badge-icons")]
	#[error("volume icon badge source exceeds the {MAX_BADGE_BYTES}-byte limit")]
	BadgeSize,
	#[cfg(feature = "badge-icons")]
	#[error(
		"volume icon badge must be a supported modern ICNS or a PNG with valid framing and chunk checksums"
	)]
	BadgePng,
	#[cfg(feature = "badge-icons")]
	#[error("volume icon badge must contain one non-animated image")]
	BadgeAnimation,
	#[cfg(feature = "badge-icons")]
	#[error("volume icon badge exceeds the {limit}-pixel dimension limit", limit = badge::MAX_BADGE_DIMENSION)]
	BadgeDimensions,
	#[cfg(feature = "badge-icons")]
	#[error(
		"volume icon badge requires sRGB-compatible samples of at most eight bits without orientation metadata"
	)]
	BadgeColor,
	#[cfg(feature = "badge-icons")]
	#[error("could not decode volume icon badge PNG")]
	BadgeDecode(#[source] png::DecodingError),
	#[error("cannot reserve ICNS assembly memory")]
	Reserve(#[from] std::collections::TryReserveError),
	#[error("unsupported volume icon PNG slot {slot:?}")]
	UnsupportedSlot { slot: dmgez_config::IconSlot },
	#[error("PNG color, bit depth or orientation cannot be preserved in ICNS element {kind:02x?}")]
	SmallIconPng { kind: Kind },
	#[error("could not encode ICNS element {kind:02x?}")]
	Encode {
		kind: Kind,
		#[source]
		source: std::io::Error,
	},
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

fn validate_png(kind: Kind, icon_type: IconType, chunks: &[u8]) -> Result<(), Error> {
	validate_png_chunks(chunks, |width, height| {
		width == icon_type.pixel_width() && height == icon_type.pixel_height()
	})
	.map_err(|error| match error {
		PngError::Framing => Error::PngFraming { kind },
		PngError::Dimensions => Error::PngDimensions { kind },
		PngError::Animation => Error::UnsupportedEncoding { kind },
		PngError::Checksum => Error::PngChecksum { kind },
	})
}

enum PngError {
	Framing,
	Dimensions,
	Animation,
	Checksum,
}

fn validate_png_chunks(
	mut chunks: &[u8],
	dimensions_allowed: impl FnOnce(u32, u32) -> bool,
) -> Result<(), PngError> {
	let header = png_chunk(chunks).ok_or(PngError::Framing)?;
	if header.tag != PNG_IHDR || header.payload.len() != PNG_IHDR_BYTES {
		return Err(PngError::Framing);
	}
	let (width, rest) = header
		.payload
		.split_first_chunk()
		.ok_or(PngError::Framing)?;
	let (height, _) = rest.split_first_chunk().ok_or(PngError::Framing)?;
	if !dimensions_allowed(u32::from_be_bytes(*width), u32::from_be_bytes(*height)) {
		return Err(PngError::Dimensions);
	}

	for _ in 0..MAX_PNG_CHUNKS {
		let chunk = png_chunk(chunks).ok_or(PngError::Framing)?;
		if PNG_ANIMATION_CHUNKS.contains(&chunk.tag) {
			return Err(PngError::Animation);
		}
		let crc_end = HEADER_BYTES + chunk.payload.len();
		if crc32fast::hash(&chunks[LENGTH_BYTES..crc_end]) != chunk.expected_crc {
			return Err(PngError::Checksum);
		}
		if chunk.tag == PNG_IEND {
			return if chunk.payload.is_empty() && chunk.remainder.is_empty() {
				Ok(())
			} else {
				Err(PngError::Framing)
			};
		}
		chunks = chunk.remainder;
	}
	Err(PngError::Framing)
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
