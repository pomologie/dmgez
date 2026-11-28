use std::{io::Cursor, num::NonZeroUsize};

use dmgez_config::Project;
use tiff::encoder::{Compression, DeflateLevel, Rational, TiffEncoder, colortype::RGB8};

use crate::{
	preflight::{
		Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent, preflight,
	},
	presentation::background::retina,
};

use super::*;

const TEST_WORKING_MEMORY: usize = 1 << 20;
const BACKGROUND_POINTS: [u32; 2] = [3, 2];
const STANDARD_DPI: u32 = 72;
const ALTERNATE_DPI: u32 = 96;
const CENTIMETER_NUMERATOR: u32 = 100;
const CENTIMETER_DENOMINATOR: u32 = 254;
const RGB_CHANNELS: u32 = 3;
const GRAY_ALPHA_CHANNELS: u32 = 2;
const STANDARD_PAGES: [(u32, ResolutionUnit, u32, u32); 2] = [
	(1, ResolutionUnit::Inch, STANDARD_DPI, 1),
	(
		RETINA_SCALE,
		ResolutionUnit::Inch,
		RETINA_SCALE * STANDARD_DPI,
		1,
	),
];

fn execution_profile() -> ExecutionProfile {
	let project = Project::parse(
		r#"schema_version = 1
[image]
name = 'Prebuilt background'
output = 'prebuilt.dmg'
format = 'udro'
"#,
	)
	.unwrap();
	preflight(
		&project,
		PreflightOptions {
			capabilities: BuildCapabilities {
				hfsplus_ascii: Availability::Available,
				..BuildCapabilities::default()
			},
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

fn tiff(pages: &[(u32, ResolutionUnit, u32, u32)]) -> Vec<u8> {
	tiff_with_compression(pages, Compression::Uncompressed)
}

fn tiff_with_compression(
	pages: &[(u32, ResolutionUnit, u32, u32)],
	compression: Compression,
) -> Vec<u8> {
	let mut bytes = Vec::new();
	let mut encoder = TiffEncoder::new(Cursor::new(&mut bytes))
		.unwrap()
		.with_compression(compression);
	for &(scale, unit, numerator, denominator) in pages {
		let [width, height] = BACKGROUND_POINTS.map(|points| points * scale);
		let mut image = encoder.new_image::<RGB8>(width, height).unwrap();
		image.resolution(
			unit,
			Rational {
				n: numerator,
				d: denominator,
			},
		);
		image
			.encoder()
			.write_tag(Tag::IccProfile, &[0_u8; 8][..])
			.unwrap();
		image
			.write_data(&vec![
				0;
				usize::try_from(width * height * RGB_CHANNELS).unwrap()
			])
			.unwrap();
	}
	bytes
}

fn gray_alpha_png(scale: u32) -> Vec<u8> {
	let [width, height] = BACKGROUND_POINTS.map(|points| points * scale);
	let mut bytes = Vec::new();
	let mut encoder = png::Encoder::new(&mut bytes, width, height);
	encoder.set_color(png::ColorType::GrayscaleAlpha);
	let mut writer = encoder.write_header().unwrap();
	writer
		.write_image_data(&vec![
			63;
			usize::try_from(width * height * GRAY_ALPHA_CHANNELS)
				.unwrap()
		])
		.unwrap();
	writer.finish().unwrap();
	bytes
}

#[test]
fn admits_generated_grayscale_alpha_as_prebuilt() {
	let profile = execution_profile();
	let bytes = retina::encode(
		&gray_alpha_png(1),
		&gray_alpha_png(RETINA_SCALE),
		TEST_WORKING_MEMORY,
		TEST_WORKING_MEMORY,
		&profile,
	)
	.unwrap();
	validate(&bytes, TEST_WORKING_MEMORY, &profile).unwrap();
}

#[test]
fn admits_reversed_pages_and_exact_cross_unit_resolution() {
	let bytes = tiff(&[
		(
			RETINA_SCALE,
			ResolutionUnit::Centimeter,
			ALTERNATE_DPI * RETINA_SCALE * CENTIMETER_NUMERATOR,
			CENTIMETER_DENOMINATOR,
		),
		(1, ResolutionUnit::Inch, ALTERNATE_DPI, 1),
	]);
	assert!(is_tiff(&bytes));
	validate(&bytes, TEST_WORKING_MEMORY, &execution_profile()).unwrap();
}

#[test]
fn admits_single_page_tiff_with_physical_resolution() {
	let bytes = tiff(&STANDARD_PAGES[..1]);
	validate(&bytes, TEST_WORKING_MEMORY, &execution_profile()).unwrap();
}

#[test]
fn admits_single_page_deflate_tiff_without_physical_resolution_units() {
	let bytes = tiff_with_compression(
		&[(1, ResolutionUnit::None, 1, 1)],
		Compression::Deflate(DeflateLevel::default()),
	);
	validate(&bytes, TEST_WORKING_MEMORY, &execution_profile()).unwrap();
}

#[test]
fn rejects_ordinary_multipage_tiff_with_identical_page_resolutions() {
	let bytes = tiff(&[
		(1, ResolutionUnit::Inch, STANDARD_DPI, 1),
		(RETINA_SCALE, ResolutionUnit::Inch, STANDARD_DPI, 1),
	]);
	assert!(matches!(
		validate(&bytes, TEST_WORKING_MEMORY, &execution_profile()),
		Err(Error::BackgroundTiffRepresentation)
	));
}

#[test]
fn rejects_an_out_of_file_metadata_payload_before_decoding() {
	let mut bytes = tiff(&STANDARD_PAGES);
	let (entry, _) = entry_in_first_directory(&mut bytes, Tag::IccProfile);
	entry[DIRECTORY_ENTRY_VALUE_OFFSET..].fill(u8::MAX);
	assert!(matches!(
		validate(&bytes, TEST_WORKING_MEMORY, &execution_profile()),
		Err(Error::BackgroundTiffRepresentation)
	));
}

fn entry_in_first_directory(
	bytes: &mut [u8],
	tag: Tag,
) -> (&mut [u8; DIRECTORY_ENTRY_BYTES], ByteOrder) {
	let order = ByteOrder::parse(bytes).unwrap();
	let directory = usize::try_from(order.u32(&bytes[FIRST_IFD_OFFSET..]).unwrap()).unwrap();
	let count = usize::from(order.u16(&bytes[directory..]).unwrap());
	let start = directory + size_of::<u16>();
	let entry = bytes[start..][..count * DIRECTORY_ENTRY_BYTES]
		.as_chunks_mut::<DIRECTORY_ENTRY_BYTES>()
		.0
		.iter_mut()
		.find(|entry| order.u16(&entry[..]) == Some(tag.to_u16()))
		.unwrap();
	(entry, order)
}

#[test]
fn rejects_raw_and_deflate_reads_past_the_declared_strip() {
	const SHORT_STRIP_BYTE_COUNT: u32 = 1;
	let profile = execution_profile();
	for compression in [
		Compression::Uncompressed,
		Compression::Deflate(DeflateLevel::default()),
	] {
		let mut bytes = tiff_with_compression(&STANDARD_PAGES, compression);
		validate(&bytes, TEST_WORKING_MEMORY, &profile).unwrap();
		let (entry, order) = entry_in_first_directory(&mut bytes, Tag::StripByteCounts);
		assert_eq!(
			order.u16(&entry[DIRECTORY_ENTRY_TYPE_OFFSET..]),
			Some(Type::LONG.to_u16())
		);
		let short_count = match order {
			ByteOrder::Big => SHORT_STRIP_BYTE_COUNT.to_be_bytes(),
			ByteOrder::Little => SHORT_STRIP_BYTE_COUNT.to_le_bytes(),
		};
		entry[DIRECTORY_ENTRY_VALUE_OFFSET..].copy_from_slice(&short_count);
		assert!(validate(&bytes, TEST_WORKING_MEMORY, &profile).is_err());
	}
}

#[test]
fn rejects_a_deflate_strip_with_pixels_but_an_incomplete_trailer() {
	let profile = execution_profile();
	let mut bytes = tiff_with_compression(
		&STANDARD_PAGES,
		Compression::Deflate(DeflateLevel::default()),
	);
	validate(&bytes, TEST_WORKING_MEMORY, &profile).unwrap();
	let (entry, order) = entry_in_first_directory(&mut bytes, Tag::StripByteCounts);
	let field = &mut entry[DIRECTORY_ENTRY_VALUE_OFFSET..];
	let size = order.u32(field).unwrap() - 1;
	let shortened = match order {
		ByteOrder::Big => size.to_be_bytes(),
		ByteOrder::Little => size.to_le_bytes(),
	};
	field.copy_from_slice(&shortened);
	assert!(validate(&bytes, TEST_WORKING_MEMORY, &profile).is_err());
}

#[test]
fn enforces_first_directory_metadata_budget_before_creating_the_decoder() {
	let bytes = tiff(&STANDARD_PAGES);
	assert!(matches!(
		validate_directories(&bytes, 1),
		Err(Error::BackgroundMemory)
	));
}
