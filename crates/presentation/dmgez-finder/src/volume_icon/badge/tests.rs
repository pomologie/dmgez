use std::{collections::BTreeMap, mem::size_of};

use dmgez_config::IconSlot;
use icns::IconFamily;
use png::Encoder;

use super::{super::encode_pngs, *};

const SAMPLE: [u8; RGBA_CHANNELS] = [156, 110, 200, 128];

fn rgba_png(width: u32, height: u32, pixel: [u8; RGBA_CHANNELS]) -> Vec<u8> {
	let mut bytes = Vec::new();
	{
		let mut encoder = Encoder::new(&mut bytes, width, height);
		encoder.set_color(ColorType::Rgba);
		let mut writer = encoder.write_header().unwrap();
		let raster: Vec<u8> = (0..width * height).flat_map(|_| pixel).collect();
		writer.write_image_data(&raster).unwrap();
		writer.finish().unwrap();
	}
	bytes
}

fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; RGBA_CHANNELS] {
	data.as_chunks::<RGBA_CHANNELS>().0[(y * width + x) as usize]
}

#[test]
fn encodes_every_native_slot_in_order_with_source_over_alpha() {
	const BADGE_CENTER: u32 = 736;
	const CANVAS_UNITS: u32 = 1024;
	const FRONT_WITH_BADGE: [u8; RGBA_CHANNELS] = [90, 83, 142, OPAQUE];

	let badge = rgba_png(2, 2, SAMPLE);
	let asset = encode_badge(&badge, MAX_BYTES).unwrap();
	assert_eq!(encode_badge(&badge, asset.len()).unwrap(), asset);
	assert_eq!(validate(&asset).unwrap().as_bytes(), asset);
	let family = IconFamily::read(Cursor::new(asset)).unwrap();
	assert_eq!(family.elements.len(), SLOTS.len());
	for (element, (_, icon_type)) in family.elements.iter().zip(SLOTS) {
		assert_eq!(element.ostype, icon_type.ostype());
		let image = element.decode_image().unwrap();
		let side = icon_type.pixel_width();
		assert_eq!((image.width(), image.height()), (side, side));
		assert_eq!(pixel(image.data(), side, 0, 0), [0; RGBA_CHANNELS]);
		let center = side * BADGE_CENTER / CANVAS_UNITS;
		assert_eq!(
			pixel(image.data(), side, center, center),
			FRONT_WITH_BADGE,
			"{icon_type:?}"
		);
	}
}

#[test]
fn retains_badge_aspect_ratio_and_opaque_colors() {
	const SIDE: u32 = 64;
	const BADGE: [u8; RGBA_CHANNELS] = [224, 45, 156, OPAQUE];
	let badge = BadgeRaster::decode(&rgba_png(1, 4, BADGE)).unwrap();
	let rendered = artwork::render(&badge, SIDE).unwrap();
	let painted: Vec<_> = (0..SIDE)
		.flat_map(|y| (0..SIDE).map(move |x| (x, y)))
		.filter(|&(x, y)| pixel(&rendered, SIDE, x, y) == BADGE)
		.collect();
	let (first_x, first_y) = painted[0];
	let (last_x, last_y) = painted[painted.len() - 1];
	// 448/1024 of a 64 pixel slot rounds to 28 pixels.
	assert_eq!((last_x - first_x + 1, last_y - first_y + 1), (7, 28));
	assert_eq!(painted.len(), 7 * 28);
}

#[test]
fn selects_largest_modern_icns_representation_independently_of_record_order() {
	const SAMPLES_32_STANDARD: [u8; RGBA_CHANNELS] = [224, 45, 156, OPAQUE];
	const SAMPLES_64_RETINA: [u8; RGBA_CHANNELS] = [16, 92, 160, 105];
	let mut sources = BTreeMap::from([
		(IconSlot::SmallRetina, rgba_png(32, 32, SAMPLE)),
		(IconSlot::Medium, rgba_png(32, 32, SAMPLES_32_STANDARD)),
	]);

	let icon = encode_pngs(&sources, MAX_BYTES).unwrap();
	let tied = BadgeRaster::decode(&icon).unwrap();
	assert_eq!((tied.width, tied.height), (32, 32));
	assert_eq!(tied.pixel(0, 0), SAMPLES_32_STANDARD);

	sources.insert(IconSlot::MediumRetina, rgba_png(64, 64, SAMPLES_64_RETINA));
	let icon = encode_pngs(&sources, MAX_BYTES).unwrap();
	let mut family = IconFamily::read(Cursor::new(icon)).unwrap();
	family.elements.reverse();
	let mut reversed = Vec::new();
	family.write(&mut reversed).unwrap();
	let largest = BadgeRaster::decode(&reversed).unwrap();
	assert_eq!((largest.width, largest.height), (64, 64));
	assert_eq!(largest.pixel(0, 0), SAMPLES_64_RETINA);
	assert!(matches!(
		BadgeRaster::decode(&reversed[..HEADER_BYTES]),
		Err(Error::FileLength)
	));
}

#[test]
fn rejects_truncated_oversized_animated_and_unknown_color_pngs() {
	let badge = rgba_png(2, 2, SAMPLE);
	assert!(matches!(
		encode_badge(&badge[..badge.len() - 1], MAX_BYTES),
		Err(Error::BadgePng)
	));
	assert!(matches!(
		encode_badge(&rgba_png(MAX_BADGE_DIMENSION + 1, 1, SAMPLE), MAX_BYTES),
		Err(Error::BadgeDimensions)
	));
	for (tag, contents) in [
		(*b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0][..]),
		(*b"iCCP", b"profile\0\0test"),
		(*b"eXIf", b"orientation"),
	] {
		const FIRST_ANCILLARY: usize = 33;
		let mut source = badge.clone();
		let mut chunk = (contents.len() as u32).to_be_bytes().to_vec();
		chunk.extend_from_slice(&tag);
		chunk.extend_from_slice(contents);
		let crc = crc32fast::hash(&chunk[size_of::<u32>()..]).to_be_bytes();
		chunk.extend_from_slice(&crc);
		source.splice(FIRST_ANCILLARY..FIRST_ANCILLARY, chunk);
		let result = encode_badge(&source, MAX_BYTES);
		if tag == *b"acTL" {
			assert!(matches!(result, Err(Error::BadgeAnimation)));
		} else {
			assert!(matches!(result, Err(Error::BadgeColor)));
		}
	}
}

#[test]
fn rejects_excess_source_and_output_bytes() {
	assert!(badge_memory(MAX_BADGE_BYTES).is_some());
	assert!(badge_memory(MAX_BADGE_BYTES + 1).is_none());
	assert!(matches!(
		encode_badge(&vec![0; MAX_BADGE_BYTES + 1], MAX_BYTES),
		Err(Error::BadgeSize)
	));
	assert!(matches!(
		encode_badge(&rgba_png(2, 2, SAMPLE), HEADER_BYTES),
		Err(Error::OutputBudget)
	));
}
