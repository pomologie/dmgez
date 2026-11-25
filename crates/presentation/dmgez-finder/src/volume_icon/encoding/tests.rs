use std::{collections::BTreeMap, io::Cursor, mem::size_of};

use dmgez_config::IconSlot;
use icns::{IconFamily, Image, PixelFormat};

use super::{super::validate, Error, HEADER_BYTES, MAX_BYTES, SLOTS, encode_pngs};

fn png(side: u32, pixel: [u8; 4]) -> Vec<u8> {
	let mut image = Image::new(PixelFormat::RGBA, side, side);
	for sample in image.data_mut().chunks_exact_mut(pixel.len()) {
		sample.copy_from_slice(&pixel);
	}
	let mut bytes = Vec::new();
	image.write_png(&mut bytes).unwrap();
	bytes
}

fn inject_png_chunk(png: &mut Vec<u8>, tag: [u8; 4], bytes: &[u8]) {
	const PNG_FIRST_ANCILLARY_OFFSET: usize = 33;
	let mut chunk = u32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
	chunk.extend_from_slice(&tag);
	chunk.extend_from_slice(bytes);
	let checksum = crc32fast::hash(&chunk[size_of::<u32>()..]);
	chunk.extend_from_slice(&checksum.to_be_bytes());
	png.splice(
		PNG_FIRST_ANCILLARY_OFFSET..PNG_FIRST_ANCILLARY_OFFSET,
		chunk,
	);
}

#[test]
fn keeps_small_native_samples_and_larger_png_payloads() {
	let source: BTreeMap<_, _> = SLOTS
		.into_iter()
		.enumerate()
		.rev()
		.map(|(index, (slot, icon_type))| {
			let sample = [19 + u8::try_from(index).unwrap(), 80, 170, 142];
			(slot, png(icon_type.pixel_width(), sample))
		})
		.collect();
	let asset = encode_pngs(&source, MAX_BYTES).unwrap();
	assert_eq!(validate(&asset).unwrap().as_bytes(), asset);
	let family = IconFamily::read(Cursor::new(&asset)).unwrap();
	assert_eq!(family.elements.len(), SLOTS.len());
	for (element, (slot, icon_type)) in family.elements.iter().zip(SLOTS) {
		assert_eq!(element.ostype, icon_type.ostype());
		if matches!(slot, IconSlot::Small | IconSlot::Medium) {
			assert!(element.data.starts_with(&super::super::ARGB_MAGIC));
			let original = Image::read_png(Cursor::new(&source[&slot])).unwrap();
			let decoded = element.decode_image().unwrap();
			assert_eq!(decoded.data(), original.data());
		} else {
			assert_eq!(element.data, source[&slot]);
		}
	}
}

#[test]
fn rejects_mis_sized_slots_and_over_budget_output() {
	const SAMPLE: [u8; 4] = [19, 80, 170, 142];
	let source = BTreeMap::from([(IconSlot::SmallRetina, png(16, SAMPLE))]);
	assert!(matches!(
		encode_pngs(&source, MAX_BYTES),
		Err(Error::PngDimensions { .. })
	));
	let source = BTreeMap::from([(IconSlot::SmallRetina, png(32, SAMPLE))]);
	assert!(matches!(
		encode_pngs(&source, HEADER_BYTES),
		Err(Error::OutputBudget)
	));
	let asset = encode_pngs(&source, MAX_BYTES).unwrap();
	assert_eq!(encode_pngs(&source, asset.len()).unwrap(), asset);
}

#[test]
fn small_argb_rejects_lost_metadata_and_short_duplicate_headers() {
	const SAMPLE: [u8; 4] = [19, 80, 170, 142];
	for (tag, bytes) in [
		(*b"iCCP", b"profile\0\0test".as_slice()),
		(*b"gAMA", &[0, 0, 180, 0]),
		(*b"IHDR", &[]),
	] {
		let mut input = png(16, SAMPLE);
		inject_png_chunk(&mut input, tag, bytes);
		let source = BTreeMap::from([(IconSlot::Small, input)]);
		assert!(matches!(
			encode_pngs(&source, MAX_BYTES),
			Err(Error::SmallIconPng { .. })
		));
	}
}
