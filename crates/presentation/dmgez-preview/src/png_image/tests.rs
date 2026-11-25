use std::cell::Cell;

use super::*;

#[test]
fn decodes_packed_opaque_and_translucent_pixels() {
	let mut rgba = Vec::new();
	let mut encoder = png::Encoder::new(&mut rgba, 2, 1);
	encoder.set_color(ColorType::Rgba);
	let samples = [23, 77, 129, 255, 29, 80, 129, 123];
	let mut writer = encoder.write_header().unwrap();
	writer.write_image_data(&samples).unwrap();
	writer.finish().unwrap();
	let image = decode_file(&rgba, || false).unwrap();
	assert_eq!(image.size(), [2, 1]);
	assert_eq!(
		image.rgba_pixels().collect::<Vec<_>>(),
		[[23, 77, 129, 255], [29, 80, 129, 123],]
	);

	let mut indexed = Vec::new();
	let mut encoder = png::Encoder::new(&mut indexed, 2, 1);
	encoder.set_color(ColorType::Indexed);
	encoder.set_palette(vec![5, 11, 33, 7, 14, 222]);
	encoder.set_trns(vec![255, 45]);
	let mut writer = encoder.write_header().unwrap();
	writer.write_image_data(&[1, 0]).unwrap();
	writer.finish().unwrap();
	let pixels: Vec<_> = decode_file(&indexed, || false)
		.unwrap()
		.rgba_pixels()
		.collect();
	assert_eq!(pixels, [[7, 14, 222, 45], [5, 11, 33, 255]]);
}

#[test]
fn refuses_encoded_and_decoded_dimensions_before_allocating_frames() {
	let input = vec![0; FILE_INPUT_BYTES + 1];
	assert!(matches!(
		decode_file(&input, || false),
		Err(Error::InputBytes)
	));
	for [width, height] in [[FILE_SIDE + 1, 1], [FILE_SIDE, FILE_PIXELS / FILE_SIDE + 1]] {
		let mut input = Vec::new();
		let encoder = png::Encoder::new(&mut input, width as u32, height as u32);
		drop(encoder.write_header().unwrap());
		assert!(matches!(
			decode_file(&input, || false),
			Err(Error::Dimensions)
		));
	}
}

#[test]
fn refuses_animation_incomplete_frames_and_trailing_data() {
	let mut animated = Vec::new();
	let mut encoder = png::Encoder::new(&mut animated, 1, 1);
	encoder.set_animated(1, 0).unwrap();
	let mut writer = encoder.write_header().unwrap();
	writer.write_image_data(&[1]).unwrap();
	writer.finish().unwrap();
	assert!(matches!(
		decode_file(&animated, || false),
		Err(Error::Profile)
	));

	let mut valid = gray();
	assert_eq!(
		decode_file(&valid, || false).unwrap().rgba_pixels().next(),
		Some([7, 7, 7, 255])
	);
	assert!(matches!(
		decode_file(&valid[..valid.len() - 1], || false),
		Err(Error::Decode(_))
	));
	valid.push(0);
	assert!(matches!(
		decode_file(&valid, || false),
		Err(Error::TrailingBytes)
	));
}

#[test]
fn checks_for_cancellation_during_decode() {
	let image = gray();
	let calls = Cell::new(0);
	let result = decode_file(&image, || {
		calls.set(calls.get() + 1);
		calls.get() >= 4
	});
	assert!(matches!(result, Err(Error::Cancelled)));
}

fn gray() -> Vec<u8> {
	let mut input = Vec::new();
	let mut writer = png::Encoder::new(&mut input, 1, 1).write_header().unwrap();
	writer.write_image_data(&[7]).unwrap();
	writer.finish().unwrap();
	input
}
