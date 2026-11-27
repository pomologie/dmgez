use std::num::NonZeroUsize;

use dmgez_config::Project;
use png::{Encoder, Info, ScaledFloat};
use tiff::{
	decoder::{Decoder, DecodingResult},
	tags::{CompressionMethod, PhotometricInterpretation},
};

use crate::preflight::{
	Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent, preflight,
};

use super::*;

const MEMORY_BYTES: usize = 4 << 20;
const METADATA_BYTES: usize = 4 << 10;

fn profile() -> ExecutionProfile {
	let project = Project::parse(
		"schema_version = 1\n[image]\nname = 'Retina codec test'\noutput = 'out.dmg'\nformat = 'udro'\n",
	)
	.unwrap();
	let capabilities = BuildCapabilities {
		hfsplus_ascii: Availability::Available,
		..BuildCapabilities::default()
	};
	preflight(
		&project,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

fn png(info: Info<'_>, pixels: &[u8]) -> Vec<u8> {
	let mut bytes = Vec::new();
	let mut writer = Encoder::with_info(&mut bytes, info)
		.unwrap()
		.write_header()
		.unwrap();
	writer.write_image_data(pixels).unwrap();
	writer.finish().unwrap();
	bytes
}

#[test]
fn transcodes_low_bit_indexed_and_sixteen_bit_gray_alpha_losslessly() {
	let mut indexed = Info::with_size(1, 1);
	indexed.color_type = ColorType::Indexed;
	indexed.bit_depth = BitDepth::One;
	indexed.palette = Some([0, 0, 0, 32, 64, 128].as_slice().into());
	indexed.trns = Some([0, 192].as_slice().into());
	let base = png(indexed, &[0b1000_0000]);

	let mut gray = Info::with_size(2, 2);
	gray.color_type = ColorType::GrayscaleAlpha;
	gray.bit_depth = BitDepth::Sixteen;
	let gray_sample: [u16; 8] = [257, 12_345, 9021, 2, 17, u16::MAX, 37_000, 0];
	let retina = png(
		gray,
		&gray_sample
			.into_iter()
			.flat_map(u16::to_be_bytes)
			.collect::<Vec<_>>(),
	);

	let result = encode(&base, &retina, MEMORY_BYTES, METADATA_BYTES, &profile()).unwrap();
	let mut tiff = Decoder::new(Cursor::new(&result)).unwrap();
	assert_eq!(tiff.dimensions().unwrap(), (1, 1));
	assert_eq!(
		tiff.get_tag_u32_vec(Tag::XResolution).unwrap(),
		[BASE_DPI, 1]
	);
	assert_eq!(
		tiff.get_tag_u32_vec(Tag::YResolution).unwrap(),
		[BASE_DPI, 1]
	);
	assert_eq!(
		tiff.get_tag_u32(Tag::Compression).unwrap(),
		u32::from(CompressionMethod::None.to_u16())
	);
	assert_eq!(
		tiff.get_tag_u32_vec(Tag::ExtraSamples).unwrap(),
		[u32::from(ExtraSamples::UnassociatedAlpha.to_u16())]
	);
	assert!(
		matches!(tiff.read_image().unwrap(), DecodingResult::U8(pixels) if pixels == [32, 64, 128, 192])
	);

	tiff.next_image().unwrap();
	assert_eq!(tiff.dimensions().unwrap(), (2, 2));
	assert_eq!(
		tiff.get_tag_u32_vec(Tag::XResolution).unwrap(),
		[RETINA_DPI, 1]
	);
	assert_eq!(
		tiff.get_tag_u32_vec(Tag::YResolution).unwrap(),
		[RETINA_DPI, 1]
	);
	assert_eq!(
		tiff.get_tag_u32(Tag::PhotometricInterpretation).unwrap(),
		u32::from(PhotometricInterpretation::BlackIsZero.to_u16())
	);
	assert!(
		matches!(tiff.read_image().unwrap(), DecodingResult::U16(pixels) if pixels == gray_sample)
	);
	assert!(!tiff.more_images());
}

#[test]
fn validates_pair_dimensions_and_refuses_uncaptured_color_semantics() {
	let base = png(Info::with_size(1, 1), &[255]);
	let mut gamma = Info::with_size(2, 2);
	gamma.source_gamma = Some(ScaledFloat::new(1.0));
	let retina = png(gamma, &[12, 13, 14, 15]);
	let profile = profile();

	assert!(matches!(
		encode(&base, &base, MEMORY_BYTES, METADATA_BYTES, &profile),
		Err(Error::BackgroundRetinaDimensions)
	));
	assert!(matches!(
		encode(&base, &retina, MEMORY_BYTES, METADATA_BYTES, &profile),
		Err(Error::BackgroundRetinaPngFormat)
	));
}

#[test]
fn accepts_upright_exif_without_changing_retina_samples() {
	// The PaintCraft artwork's EXIF has an ExifIFD pointer and no orientation.
	const DIMENSIONS_ONLY: &[u8] = b"MM\x00*\x00\x00\x00\x08\x00\x01\x87\x69\x00\x04\
	\x00\x00\x00\x01\x00\x00\x00\x1a\x00\x00\x00\x00\x00\x02\xa0\x02\x00\x04\
	\x00\x00\x00\x01\x00\x00\x02\x80\xa0\x03\x00\x04\x00\x00\x00\x01\x00\x00\x00\xa0\
	\x00\x00\x00\x00";
	const UPRIGHT: &[u8] = b"II*\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\
	\x01\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00";
	const ROTATED: &[u8] = b"MM\x00*\x00\x00\x00\x08\x00\x01\x01\x12\x00\x03\
	\x00\x00\x00\x01\x00\x06\x00\x00\x00\x00\x00\x00";
	let mut base = Info::with_size(1, 1);
	base.exif_metadata = Some(DIMENSIONS_ONLY.into());
	let base = png(base, &[42]);
	let samples = [12, 13, 14, 15];
	let mut retina = Info::with_size(2, 2);
	retina.exif_metadata = Some(UPRIGHT.into());
	let profile = profile();
	let result = encode(
		&base,
		&png(retina.clone(), &samples),
		MEMORY_BYTES,
		METADATA_BYTES,
		&profile,
	)
	.unwrap();
	let mut tiff = Decoder::new(Cursor::new(result)).unwrap();
	assert!(matches!(tiff.read_image().unwrap(), DecodingResult::U8(pixels) if pixels == [42]));
	tiff.next_image().unwrap();
	assert!(matches!(tiff.read_image().unwrap(), DecodingResult::U8(pixels) if pixels == samples));
	retina.exif_metadata = Some(ROTATED.into());
	assert!(matches!(
		encode(
			&base,
			&png(retina, &samples),
			MEMORY_BYTES,
			METADATA_BYTES,
			&profile
		),
		Err(Error::BackgroundRetinaPngFormat)
	));
}

#[test]
fn enforces_separate_working_and_output_caps() {
	let base = png(Info::with_size(1, 1), &[0]);
	let retina = png(Info::with_size(2, 2), &[12, 13, 14, 15]);
	let profile = profile();
	let result = encode(&base, &retina, MEMORY_BYTES, METADATA_BYTES, &profile).unwrap();

	assert!(matches!(
		encode(&base, &retina, 0, METADATA_BYTES, &profile),
		Err(Error::BackgroundMemory)
	));
	assert!(matches!(
		encode(&base, &retina, MEMORY_BYTES, result.len() - 1, &profile),
		Err(Error::MetadataBudget)
	));
	assert_eq!(
		encode(&base, &retina, MEMORY_BYTES, result.len(), &profile).unwrap(),
		result
	);
}
