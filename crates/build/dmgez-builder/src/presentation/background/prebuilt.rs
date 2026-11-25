use std::mem::size_of;

use tiff::{
	ColorType, TiffError,
	decoder::{Decoder, Limits, ifd::Value},
	tags::{
		CompressionMethod, ExtraSamples, PhotometricInterpretation, PlanarConfiguration,
		ResolutionUnit, SampleFormat, Tag, Type,
	},
};

use crate::preflight::ExecutionProfile;

use super::{
	Error, MAX_IMAGE_DIMENSION, MAX_RASTER_BYTES, RETINA_SCALE, TIFF_TOP_LEFT, check_cancelled,
};
use strips::{ImageDecoder, StripSource, StripSpan, validate_strips};

mod strips;

const LITTLE_ENDIAN_MARKER: &[u8] = b"II";
const BIG_ENDIAN_MARKER: &[u8] = b"MM";
const CLASSIC_TIFF_VERSION: u16 = 42;
const BIG_TIFF_VERSION: u16 = 43;
const FIRST_IFD_OFFSET: usize = 4;
const CLASSIC_HEADER_BYTES: usize = 8;
const DIRECTORY_ENTRY_BYTES: usize = 12;
const DIRECTORY_ENTRY_TYPE_OFFSET: usize = 2;
const DIRECTORY_ENTRY_COUNT_OFFSET: usize = 4;
const DIRECTORY_ENTRY_VALUE_OFFSET: usize = 8;
const MAX_DIRECTORY_ENTRIES: usize = 128;
// Allow for both directory maps during a page transition, plus codec state.
const DECODER_OVERHEAD_BYTES: usize = 128 << 10;
const DECODER_BUDGET_DIVISOR: usize = 4;
// An input value may exist in both directories and in the decoder's original
// and converted vectors while the second directory is opened.
const BYTES_PER_IFD_VALUE: usize = 4 * (size_of::<Value>() + size_of::<u64>());
const CENTIMETERS_PER_INCH_NUMERATOR: u128 = 254;
const CENTIMETERS_PER_INCH_DENOMINATOR: u128 = 100;

#[derive(Clone, Copy)]
enum ByteOrder {
	Little,
	Big,
}

impl ByteOrder {
	fn parse(bytes: &[u8]) -> Option<Self> {
		if bytes.starts_with(LITTLE_ENDIAN_MARKER) {
			Some(Self::Little)
		} else if bytes.starts_with(BIG_ENDIAN_MARKER) {
			Some(Self::Big)
		} else {
			None
		}
	}

	fn version(self, bytes: &[u8]) -> Option<u16> {
		self.u16(bytes.get(LITTLE_ENDIAN_MARKER.len()..)?)
	}

	fn u16(self, bytes: &[u8]) -> Option<u16> {
		let value = bytes.first_chunk().copied()?;
		Some(match self {
			Self::Little => u16::from_le_bytes(value),
			Self::Big => u16::from_be_bytes(value),
		})
	}

	fn u32(self, bytes: &[u8]) -> Option<u32> {
		let value = bytes.first_chunk().copied()?;
		Some(match self {
			Self::Little => u32::from_le_bytes(value),
			Self::Big => u32::from_be_bytes(value),
		})
	}
}

pub(super) fn is_tiff(bytes: &[u8]) -> bool {
	matches!(
		ByteOrder::parse(bytes).and_then(|order| order.version(bytes)),
		Some(CLASSIC_TIFF_VERSION | BIG_TIFF_VERSION)
	)
}

pub(super) fn validate(
	bytes: &[u8],
	available: usize,
	profile: &ExecutionProfile,
) -> Result<(), Error> {
	check_cancelled(profile)?;
	let available = available
		.checked_sub(DECODER_OVERHEAD_BYTES)
		.ok_or(Error::BackgroundMemory)?;
	let values = available / DECODER_BUDGET_DIVISOR;
	let working = available / DECODER_BUDGET_DIVISOR;
	let raster_limit = (available - values - working).min(MAX_RASTER_BYTES);

	validate_directories(bytes, values / BYTES_PER_IFD_VALUE)?;
	let mut limits = Limits::default();
	limits.ifd_value_size = values;
	limits.intermediate_buffer_size = working;
	limits.decoding_buffer_size = raster_limit;
	let span = StripSpan::default();
	let mut decoder = Decoder::new(StripSource::new(bytes, &span))
		.map_err(tiff_error)?
		.with_limits(limits);
	let first = Representation::read(&mut decoder, raster_limit)?;
	validate_strips(&mut decoder, &span, bytes, raster_limit, profile)?;
	decoder.next_image().map_err(tiff_error)?;
	let second = Representation::read(&mut decoder, raster_limit)?;
	let (base, retina) = if first.width < second.width {
		(first, second)
	} else {
		(second, first)
	};
	if !base.matches_retina(retina) {
		return Err(Error::BackgroundTiffRepresentation);
	}
	validate_strips(&mut decoder, &span, bytes, raster_limit, profile)?;
	check_cancelled(profile)
}

fn validate_directories(bytes: &[u8], max_values: usize) -> Result<(), Error> {
	let order = ByteOrder::parse(bytes)
		.filter(|&order| order.version(bytes) == Some(CLASSIC_TIFF_VERSION))
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let first_ifd = bytes
		.get(FIRST_IFD_OFFSET..)
		.and_then(|field| order.u32(field))
		.and_then(|value| usize::try_from(value).ok())
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let second_ifd = validate_directory(bytes, first_ifd, order, max_values)?;
	if first_ifd == second_ifd || validate_directory(bytes, second_ifd, order, max_values)? != 0 {
		return Err(Error::BackgroundTiffRepresentation);
	}
	Ok(())
}

fn validate_directory(
	bytes: &[u8],
	offset: usize,
	order: ByteOrder,
	max_values: usize,
) -> Result<usize, Error> {
	// Decoder::new parses the first directory before caller-provided limits
	// apply, so bound the IFD entries and eagerly decoded values first.
	let directory = (offset >= CLASSIC_HEADER_BYTES)
		.then(|| bytes.get(offset..))
		.flatten()
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let entries_count = order
		.u16(directory)
		.map(usize::from)
		.filter(|count| (1..=MAX_DIRECTORY_ENTRIES).contains(count))
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let entries_end = size_of::<u16>() + entries_count * DIRECTORY_ENTRY_BYTES;
	let entries = directory
		.get(size_of::<u16>()..entries_end)
		.ok_or(Error::BackgroundTiffRepresentation)?;

	let mut previous_tag = None;
	let mut eager_values = 0;
	for entry in entries.as_chunks::<DIRECTORY_ENTRY_BYTES>().0 {
		let tag_code = order.u16(entry).expect("complete directory entry");
		if previous_tag.is_some_and(|previous| previous >= tag_code) {
			return Err(Error::BackgroundTiffRepresentation);
		}
		previous_tag = Some(tag_code);
		let tag = Tag::from_u16_exhaustive(tag_code);
		if matches!(
			tag,
			Tag::TileWidth | Tag::TileLength | Tag::TileOffsets | Tag::TileByteCounts | Tag::SubIfd
		) {
			return Err(Error::BackgroundTiffRepresentation);
		}
		let (kind, count, payload) = entry_payload(bytes, entry, order)?;
		if matches!(
			tag,
			Tag::ImageWidth | Tag::ImageLength | Tag::Compression | Tag::PlanarConfiguration
		) {
			validate_scalar(tag, kind, count, payload, order)?;
		}
		if decoder_reads(tag) {
			eager_values = count
				.checked_add(eager_values)
				.filter(|&total| total <= max_values)
				.ok_or(Error::BackgroundMemory)?;
		}
	}
	order
		.u32(&directory[entries_end..])
		.and_then(|value| usize::try_from(value).ok())
		.ok_or(Error::BackgroundTiffRepresentation)
}

fn entry_payload<'a>(
	bytes: &'a [u8],
	entry: &'a [u8],
	order: ByteOrder,
) -> Result<(Type, usize, &'a [u8]), Error> {
	let kind = order
		.u16(&entry[DIRECTORY_ENTRY_TYPE_OFFSET..])
		.and_then(Type::from_u16)
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let count = order
		.u32(&entry[DIRECTORY_ENTRY_COUNT_OFFSET..])
		.and_then(|value| usize::try_from(value).ok())
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let wire_width = match kind {
		Type::BYTE | Type::SBYTE | Type::ASCII | Type::UNDEFINED => size_of::<u8>(),
		Type::SHORT | Type::SSHORT => size_of::<u16>(),
		Type::LONG | Type::SLONG | Type::FLOAT | Type::IFD => size_of::<u32>(),
		Type::RATIONAL | Type::SRATIONAL | Type::DOUBLE => size_of::<u64>(),
		_ => return Err(Error::BackgroundTiffRepresentation),
	};
	let len = count
		.checked_mul(wire_width)
		.ok_or(Error::BackgroundTiffRepresentation)?;
	let field = &entry[DIRECTORY_ENTRY_VALUE_OFFSET..];
	let payload = if len <= field.len() {
		&field[..len]
	} else {
		let offset = order
			.u32(field)
			.and_then(|value| usize::try_from(value).ok())
			.filter(|&value| value >= CLASSIC_HEADER_BYTES)
			.ok_or(Error::BackgroundTiffRepresentation)?;
		let end = offset
			.checked_add(len)
			.ok_or(Error::BackgroundTiffRepresentation)?;
		bytes
			.get(offset..end)
			.ok_or(Error::BackgroundTiffRepresentation)?
	};
	Ok((kind, count, payload))
}

fn decoder_reads(tag: Tag) -> bool {
	matches!(
		tag,
		Tag::ImageWidth
			| Tag::ImageLength
			| Tag::PhotometricInterpretation
			| Tag::Compression
			| Tag::SamplesPerPixel
			| Tag::ExtraSamples
			| Tag::SampleFormat
			| Tag::BitsPerSample
			| Tag::Predictor
			| Tag::PlanarConfiguration
			| Tag::ChromaSubsampling
			| Tag::StripOffsets
			| Tag::StripByteCounts
			| Tag::RowsPerStrip
			| Tag::Orientation
			| Tag::ResolutionUnit
			| Tag::XResolution
			| Tag::YResolution
	)
}

fn validate_scalar(
	tag: Tag,
	kind: Type,
	count: usize,
	payload: &[u8],
	order: ByteOrder,
) -> Result<(), Error> {
	let value = match (kind, count) {
		(Type::SHORT, 1) => order.u16(payload).map(u32::from),
		(Type::LONG, 1) if matches!(tag, Tag::ImageWidth | Tag::ImageLength) => order.u32(payload),
		_ => None,
	}
	.ok_or(Error::BackgroundTiffRepresentation)?;
	match tag {
		Tag::ImageWidth | Tag::ImageLength if value > MAX_IMAGE_DIMENSION => {
			Err(Error::BackgroundMemory)
		}
		Tag::PlanarConfiguration if value != u32::from(PlanarConfiguration::Chunky.to_u16()) => {
			Err(Error::BackgroundTiffRepresentation)
		}
		Tag::Compression => {
			match CompressionMethod::from_u16(u16::try_from(value).expect("compression is a SHORT"))
			{
				Some(
					CompressionMethod::None
					| CompressionMethod::LZW
					| CompressionMethod::Deflate
					| CompressionMethod::OldDeflate
					| CompressionMethod::PackBits,
				) => Ok(()),
				_ => Err(Error::BackgroundTiffRepresentation),
			}
		}
		_ => Ok(()),
	}
}

#[derive(Clone, Copy)]
struct Density {
	numerator: u128,
	denominator: u128,
}

impl Density {
	fn read(decoder: &mut ImageDecoder<'_>, tag: Tag, unit: ResolutionUnit) -> Result<Self, Error> {
		let Value::Rational(numerator, denominator) = decoder.get_tag(tag).map_err(tiff_error)?
		else {
			return Err(Error::BackgroundTiffRepresentation);
		};
		if numerator == 0 || denominator == 0 {
			return Err(Error::BackgroundTiffRepresentation);
		}
		let (scale_numerator, scale_denominator) = match unit {
			ResolutionUnit::Inch => (1, 1),
			ResolutionUnit::Centimeter => (
				CENTIMETERS_PER_INCH_NUMERATOR,
				CENTIMETERS_PER_INCH_DENOMINATOR,
			),
			_ => return Err(Error::BackgroundTiffRepresentation),
		};
		Ok(Self {
			numerator: u128::from(numerator) * scale_numerator,
			denominator: u128::from(denominator) * scale_denominator,
		})
	}

	fn matches_retina(self, retina: Self) -> bool {
		// TIFF unsigned rationals are u32, so both cross products, including
		// the exact centimeter conversion, fit comfortably in u128.
		retina.numerator * self.denominator
			== self.numerator * retina.denominator * u128::from(RETINA_SCALE)
	}
}

struct Representation {
	width: u32,
	height: u32,
	horizontal: Density,
	vertical: Density,
}

impl Representation {
	fn read(decoder: &mut ImageDecoder<'_>, raster_limit: usize) -> Result<Self, Error> {
		let (width, height) = decoder.dimensions().map_err(tiff_error)?;
		let layout = decoder.image_buffer_layout().map_err(tiff_error)?;
		if layout.complete_len > raster_limit {
			return Err(Error::BackgroundMemory);
		}
		if !valid_samples(decoder)?
			|| layout.sample_format != SampleFormat::Uint
			|| decoder
				.find_tag_unsigned(Tag::Orientation)
				.map_err(tiff_error)?
				.unwrap_or(TIFF_TOP_LEFT)
				!= TIFF_TOP_LEFT
		{
			return Err(Error::BackgroundTiffRepresentation);
		}
		let unit = ResolutionUnit::from_u16(
			decoder
				.find_tag_unsigned(Tag::ResolutionUnit)
				.map_err(tiff_error)?
				.unwrap_or(ResolutionUnit::Inch.to_u16()),
		)
		.ok_or(Error::BackgroundTiffRepresentation)?;
		Ok(Self {
			width,
			height,
			horizontal: Density::read(decoder, Tag::XResolution, unit)?,
			vertical: Density::read(decoder, Tag::YResolution, unit)?,
		})
	}

	fn matches_retina(self, retina: Self) -> bool {
		self.width.checked_mul(RETINA_SCALE) == Some(retina.width)
			&& self.height.checked_mul(RETINA_SCALE) == Some(retina.height)
			&& self.horizontal.matches_retina(retina.horizontal)
			&& self.vertical.matches_retina(retina.vertical)
	}
}

fn valid_samples(decoder: &mut ImageDecoder<'_>) -> Result<bool, Error> {
	let extras = decoder
		.find_tag_unsigned_vec::<u16>(Tag::ExtraSamples)
		.map_err(tiff_error)?
		.unwrap_or_default();
	let alpha = matches!(
		extras.as_slice(),
		&[kind] if matches!(
			ExtraSamples::from_u16(kind),
			Some(ExtraSamples::AssociatedAlpha | ExtraSamples::UnassociatedAlpha)
		)
	);
	Ok(match decoder.colortype().map_err(tiff_error)? {
		ColorType::Gray(8 | 16) | ColorType::RGB(8 | 16) => extras.is_empty(),
		ColorType::RGBA(8 | 16) => alpha,
		// The pinned TIFF decoder represents BlackIsZero with alpha as two
		// bands, so verify the declared alpha sample before decoding it.
		ColorType::Multiband {
			bit_depth: 8 | 16,
			num_samples: 2,
		} => {
			alpha
				&& decoder
					.get_tag_unsigned::<u16>(Tag::PhotometricInterpretation)
					.map_err(tiff_error)?
					== PhotometricInterpretation::BlackIsZero.to_u16()
		}
		_ => false,
	})
}

fn tiff_error(error: TiffError) -> Error {
	match error {
		TiffError::LimitsExceeded => Error::BackgroundMemory,
		other => Error::BackgroundTiff(other),
	}
}

#[cfg(test)]
mod tests;
