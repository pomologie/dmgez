use std::{borrow::Cow, collections::BTreeMap, io::Cursor, mem::size_of};

use dmgez_config::IconSlot;
use icns::{IconElement, IconType, Image};

use super::{
	Error, HEADER_BYTES, ICNS_MAGIC, Kind, MAX_BYTES, PNG_IHDR, PNG_MAGIC, png_chunk,
	validate_image, validate_png,
};

#[cfg(test)]
mod tests;

const PNG_DECODER_BYTES: usize = 64 << 20;
const MAX_RGBA_RASTER_BYTES: usize = 4 << 20;
// The only converted rasters are at most 32 × 32 × 4 bytes. This budget
// includes both RLE outputs and the short-lived color conversion and slot buffers.
const ARGB_ASSEMBLY_BYTES: usize = 32 << 10;
const PNG_IHDR_DEPTH_INDEX: usize = 2 * size_of::<u32>();
const MAX_ARGB_BITS_PER_SAMPLE: u8 = 8;
const MAX_SRGB_RENDERING_INTENT: u8 = 3;
const PNG_SRGB: Kind = *b"sRGB";
const PNG_GAMMA: Kind = *b"gAMA";
const PNG_CHROMATICITY: Kind = *b"cHRM";
const PNG_UNREPRESENTABLE_ARGB_METADATA: [Kind; 5] =
	[*b"iCCP", *b"cICP", *b"mDCV", *b"cLLI", *b"eXIf"];

// Apple ImageIO distinguishes ic04 and ic05 from ic11 even though the latter
// two are both 32 × 32 pixels. In Apple's iconset, ic05 is the 32-point
// standard icon and ic11 is the 16-point Retina icon.
const SLOTS: [(IconSlot, IconType); 10] = [
	(IconSlot::Small, IconType::RGBA32_16x16_ARGB),
	(IconSlot::SmallRetina, IconType::RGBA32_16x16_2x),
	(IconSlot::Medium, IconType::RGBA32_16x16_2x_ARGB),
	(IconSlot::MediumRetina, IconType::RGBA32_32x32_2x),
	(IconSlot::Large, IconType::RGBA32_128x128),
	(IconSlot::LargeRetina, IconType::RGBA32_128x128_2x),
	(IconSlot::Larger, IconType::RGBA32_256x256),
	(IconSlot::LargerRetina, IconType::RGBA32_256x256_2x),
	(IconSlot::Largest, IconType::RGBA32_512x512),
	(IconSlot::LargestRetina, IconType::RGBA32_512x512_2x),
];

/// Returns the codec working-memory allowance for validating a prebuilt ICNS.
///
/// Pass the entire encoded ICNS length. The caller budgets retained input
/// separately. Overflow returns `None`.
#[must_use]
pub fn validation_memory(icns_bytes: usize) -> Option<usize> {
	(PNG_DECODER_BYTES + MAX_RGBA_RASTER_BYTES).checked_add(icns_bytes)
}

/// Returns the codec working-memory allowance for assembling PNG icon slots.
///
/// Pass the largest encoded PNG length. The caller budgets retained input
/// and output separately. Overflow returns `None`.
#[must_use]
pub fn png_encoding_memory(largest_png_bytes: usize) -> Option<usize> {
	validation_memory(largest_png_bytes)?.checked_add(ARGB_ASSEMBLY_BYTES)
}

/// Builds an ICNS from one or more configured logical-size and scale slots.
///
/// Each PNG must have the exact physical dimensions of its slot. Standard
/// 16- and 32-point images use native planar ARGB and require color data
/// representable as eight-bit sRGB. Other slots keep their encoded PNG bytes.
/// Elements are written by ascending logical size with standard scale first.
///
/// # Errors
/// Rejects empty, malformed, animated, mis-sized or unsupported inputs,
/// allocation failures, and an ICNS exceeding the output or profile limit.
pub fn encode_pngs(
	pngs: &BTreeMap<IconSlot, impl AsRef<[u8]>>,
	max_output_bytes: usize,
) -> Result<Vec<u8>, Error> {
	if pngs.is_empty() {
		return Err(Error::NoImage);
	}
	pngs.values().try_fold(0_usize, |total, png| {
		total
			.checked_add(png.as_ref().len())
			.filter(|&bytes| bytes <= MAX_BYTES)
			.ok_or(Error::Size)
	})?;
	if let Some(&slot) = pngs
		.keys()
		.find(|&&slot| !SLOTS.iter().any(|&(key, _)| key == slot))
	{
		return Err(Error::UnsupportedSlot { slot });
	}

	let mut asset_size = HEADER_BYTES;
	let mut elements = Vec::new();
	elements.try_reserve_exact(pngs.len())?;
	for (slot, icon_type) in SLOTS {
		let Some(png) = pngs.get(&slot) else {
			continue;
		};
		let kind = icon_type.ostype().0;
		let png = png.as_ref();
		let chunks = png
			.strip_prefix(&PNG_MAGIC)
			.ok_or(Error::UnsupportedEncoding { kind })?;
		let payload = match slot {
			IconSlot::Small | IconSlot::Medium => {
				Cow::Owned(encode_small_argb(kind, icon_type, png, chunks)?)
			}
			_ => {
				validate_image(kind, png)?;
				Cow::Borrowed(png)
			}
		};
		asset_size = asset_size
			.checked_add(HEADER_BYTES + payload.len())
			.filter(|&bytes| bytes <= MAX_BYTES)
			.ok_or(Error::Size)?;
		if asset_size > max_output_bytes {
			return Err(Error::OutputBudget);
		}
		elements.push((kind, payload));
	}

	let mut asset = Vec::new();
	asset.try_reserve_exact(asset_size)?;
	if asset.capacity() > max_output_bytes {
		return Err(Error::OutputBudget);
	}
	write_header(&mut asset, ICNS_MAGIC, asset_size)?;
	for (kind, payload) in elements {
		write_header(&mut asset, kind, HEADER_BYTES + payload.len())?;
		asset.extend_from_slice(&payload);
	}
	Ok(asset)
}

fn encode_small_argb(
	kind: Kind,
	icon_type: IconType,
	png: &[u8],
	chunks: &[u8],
) -> Result<Vec<u8>, Error> {
	validate_png(kind, icon_type, chunks)?;
	validate_argb_metadata(kind, chunks)?;
	let image =
		Image::read_png(Cursor::new(png)).map_err(|source| Error::Decode { kind, source })?;
	let element = IconElement::encode_image_with_type(&image, icon_type)
		.map_err(|source| Error::Encode { kind, source })?;
	Ok(element.data)
}

fn validate_argb_metadata(kind: Kind, mut chunks: &[u8]) -> Result<(), Error> {
	let mut srgb = false;
	let mut custom_gamma_or_chromaticity = false;
	while let Some(chunk) = png_chunk(chunks) {
		if PNG_UNREPRESENTABLE_ARGB_METADATA.contains(&chunk.tag)
			|| (chunk.tag == PNG_IHDR
				&& chunk
					.payload
					.get(PNG_IHDR_DEPTH_INDEX)
					.is_none_or(|&depth| depth > MAX_ARGB_BITS_PER_SAMPLE))
		{
			return Err(Error::SmallIconPng { kind });
		}
		if chunk.tag == PNG_SRGB {
			if !matches!(chunk.payload, [intent] if *intent <= MAX_SRGB_RENDERING_INTENT) {
				return Err(Error::SmallIconPng { kind });
			}
			srgb = true;
		}
		custom_gamma_or_chromaticity |= matches!(chunk.tag, PNG_GAMMA | PNG_CHROMATICITY);
		chunks = chunk.remainder;
	}
	if custom_gamma_or_chromaticity && !srgb {
		return Err(Error::SmallIconPng { kind });
	}
	Ok(())
}

fn write_header(output: &mut Vec<u8>, kind: Kind, size: usize) -> Result<(), Error> {
	let size = u32::try_from(size).map_err(|_| Error::Size)?;
	output.extend_from_slice(&kind);
	output.extend_from_slice(&size.to_be_bytes());
	Ok(())
}
