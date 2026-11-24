#![cfg(feature = "volume-icon")]

use dmgez_finder::volume_icon::{Error, MAX_BYTES, validate};

const NATIVE: &[u8] = include_bytes!("fixtures/apple-iconutil.icns");
const ELEMENT_HEADER_BYTES: usize = 8;
const FIRST_NATIVE_PNG: [u8; 4] = *b"ic12";
const NATIVE_ARGB: [u8; 4] = *b"ic04";
const INFO: [u8; 4] = *b"info";
const PNG_SIGNATURE_BYTES: usize = 8;
const FIRST_PNG_WIDTH: usize = PNG_SIGNATURE_BYTES + ELEMENT_HEADER_BYTES;
const ROOT_FLAGS_OFFSET: usize = 8;
const ROOT_FLAGS_END: usize = ROOT_FLAGS_OFFSET + 2;
const HAS_CUSTOM_ICON: u16 = 0x0400;
const EXIF: [u8; 4] = *b"eXIf";
const IMAGE_DATA: [u8; 4] = *b"IDAT";

#[test]
fn preserves_the_complete_apple_iconutil_source_and_sets_only_the_root_flag() {
	let icon = validate(NATIVE).unwrap();
	assert_eq!(icon.as_bytes(), NATIVE);
	assert!(std::ptr::eq(icon.as_bytes().as_ptr(), NATIVE.as_ptr()));

	let original = [0x21; 32];
	let mut finder_info = original;
	let original_flags = u16::from_be_bytes(
		original[ROOT_FLAGS_OFFSET..ROOT_FLAGS_END]
			.try_into()
			.unwrap(),
	);
	icon.mark_root_finder_info(&mut finder_info);
	let expected = (original_flags | HAS_CUSTOM_ICON).to_be_bytes();
	assert_eq!(finder_info[ROOT_FLAGS_OFFSET..ROOT_FLAGS_END], expected);
	assert_eq!(
		finder_info[..ROOT_FLAGS_OFFSET],
		original[..ROOT_FLAGS_OFFSET]
	);
	assert_eq!(finder_info[ROOT_FLAGS_END..], original[ROOT_FLAGS_END..]);
	icon.mark_root_finder_info(&mut finder_info);
	assert_eq!(finder_info[ROOT_FLAGS_OFFSET..ROOT_FLAGS_END], expected);
}

#[test]
fn rejects_unbounded_or_ambiguous_icns_framing() {
	assert!(matches!(validate(b"icn"), Err(Error::FileHeader)));
	assert!(matches!(
		validate(&NATIVE[..NATIVE.len() - 1]),
		Err(Error::FileLength)
	));
	assert!(matches!(validate(&family(&[])), Err(Error::NoImage)));
	assert!(matches!(
		validate(&vec![0; MAX_BYTES + 1]),
		Err(Error::Size)
	));

	let mut oversized_element = NATIVE.to_vec();
	let field = ELEMENT_HEADER_BYTES + FIRST_NATIVE_PNG.len();
	oversized_element[field..field + size_of::<u32>()].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(matches!(
		validate(&oversized_element),
		Err(Error::ElementLength {
			offset: ELEMENT_HEADER_BYTES
		})
	));

	let png = native_element(FIRST_NATIVE_PNG);
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, png), (FIRST_NATIVE_PNG, png)])),
		Err(Error::DuplicateElement {
			kind: FIRST_NATIVE_PNG
		})
	));
	assert!(matches!(
		validate(&family(&[(*b"TOC ", png)])),
		Err(Error::UnsupportedElement { kind }) if &kind == b"TOC "
	));
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, b"arbitrary bytes")])),
		Err(Error::UnsupportedEncoding {
			kind: FIRST_NATIVE_PNG
		})
	));
}

#[test]
fn accepts_only_static_exact_png_images_with_valid_data() {
	let original = native_element(FIRST_NATIVE_PNG);
	let mut wrong_dimensions = original.to_vec();
	wrong_dimensions[FIRST_PNG_WIDTH..FIRST_PNG_WIDTH + size_of::<u32>()]
		.copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, &wrong_dimensions)])),
		Err(Error::PngDimensions {
			kind: FIRST_NATIVE_PNG
		})
	));

	let mut bad_crc = original.to_vec();
	let exif = chunk_kind_offset(original, EXIF);
	bad_crc[exif + EXIF.len()] ^= 1;
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, &bad_crc)])),
		Err(Error::PngChecksum {
			kind: FIRST_NATIVE_PNG
		})
	));

	let mut animated = original.to_vec();
	let idat = chunk_kind_offset(original, IMAGE_DATA);
	animated[idat..idat + IMAGE_DATA.len()].copy_from_slice(b"acTL");
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, &animated)])),
		Err(Error::UnsupportedEncoding {
			kind: FIRST_NATIVE_PNG
		})
	));

	let mut bad_stream = original.to_vec();
	let encoded_size = integer(original, idat - size_of::<u32>());
	bad_stream[idat + IMAGE_DATA.len()] = 0;
	let crc_end = idat + IMAGE_DATA.len() + encoded_size;
	let crc = crc32fast::hash(&bad_stream[idat..crc_end]);
	bad_stream[crc_end..crc_end + size_of::<u32>()].copy_from_slice(&crc.to_be_bytes());
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, &bad_stream)])),
		Err(Error::Decode {
			kind: FIRST_NATIVE_PNG,
			..
		})
	));

	let mut trailing = original.to_vec();
	trailing.push(0);
	assert!(matches!(
		validate(&family(&[(FIRST_NATIVE_PNG, &trailing)])),
		Err(Error::PngFraming {
			kind: FIRST_NATIVE_PNG
		})
	));
}

#[test]
fn validates_native_argb_and_limits_repeated_plist_references() {
	let argb = native_element(NATIVE_ARGB);
	let argb_only = family(&[(NATIVE_ARGB, argb)]);
	assert_eq!(validate(&argb_only).unwrap().as_bytes(), argb_only);
	assert!(matches!(
		validate(&family(&[(NATIVE_ARGB, &argb[..argb.len() - 1])])),
		Err(Error::Decode {
			kind: NATIVE_ARGB,
			..
		})
	));
	assert!(matches!(
		validate(&family(&[
			(NATIVE_ARGB, argb),
			(INFO, b"not a property list")
		])),
		Err(Error::Info)
	));
	let within_budget = shared_info_tree(4);
	assert!(validate(&family(&[(NATIVE_ARGB, argb), (INFO, &within_budget)])).is_ok());
	// A tiny encoded object table would emit more than 2^16 plist events.
	let over_budget = shared_info_tree(16);
	assert!(matches!(
		validate(&family(&[(NATIVE_ARGB, argb), (INFO, &over_budget)])),
		Err(Error::Info)
	));
}

fn family(elements: &[([u8; 4], &[u8])]) -> Vec<u8> {
	let total = ELEMENT_HEADER_BYTES
		+ elements
			.iter()
			.map(|(_, payload)| ELEMENT_HEADER_BYTES + payload.len())
			.sum::<usize>();
	let mut bytes = Vec::from(*b"icns");
	bytes.extend_from_slice(&u32::try_from(total).unwrap().to_be_bytes());
	for (kind, payload) in elements {
		bytes.extend_from_slice(kind);
		let length = u32::try_from(ELEMENT_HEADER_BYTES + payload.len()).unwrap();
		bytes.extend_from_slice(&length.to_be_bytes());
		bytes.extend_from_slice(payload);
	}
	bytes
}

fn native_element(expected: [u8; 4]) -> &'static [u8] {
	let mut rest = &NATIVE[ELEMENT_HEADER_BYTES..];
	while !rest.is_empty() {
		let length = integer(rest, expected.len());
		let (element, remainder) = rest.split_at(length);
		if element.starts_with(&expected) {
			return &element[ELEMENT_HEADER_BYTES..];
		}
		rest = remainder;
	}
	panic!("expected representation {expected:02x?}");
}

fn chunk_kind_offset(png: &[u8], expected: [u8; 4]) -> usize {
	let crc_bytes = size_of::<u32>();
	let mut offset = PNG_SIGNATURE_BYTES;
	while offset < png.len() {
		let length = integer(png, offset);
		let tag_offset = offset + size_of::<u32>();
		if png[tag_offset..].starts_with(&expected) {
			return tag_offset;
		}
		offset += ELEMENT_HEADER_BYTES + length + crc_bytes;
	}
	panic!("expected PNG chunk {expected:02x?}");
}

fn integer(bytes: &[u8], offset: usize) -> usize {
	u32::from_be_bytes(bytes[offset..offset + size_of::<u32>()].try_into().unwrap()) as usize
}

fn shared_info_tree(levels: u8) -> Vec<u8> {
	const ONE_ENTRY_DICTIONARY: u8 = 0xd1;
	const ONE_ASCII_CHARACTER: u8 = 0x51;
	const TWO_ITEM_ARRAY: u8 = 0xa2;
	const TRUE: u8 = 0x09;
	const KEY_OBJECT: u8 = 1;
	const FIRST_VALUE_OBJECT: u8 = 2;
	const ROOT_OBJECT: u64 = 0;
	const TRAILER_RESERVED_BYTES: usize = 6;
	const ONE_BYTE_OBJECT_OFFSET_AND_REFERENCE: [u8; 2] = [1, 1];

	let mut data = Vec::from(*b"bplist00");
	let mut offsets = Vec::new();
	let mut object = |encoded: &[u8]| {
		offsets.push(u8::try_from(data.len()).unwrap());
		data.extend_from_slice(encoded);
	};
	object(&[ONE_ENTRY_DICTIONARY, KEY_OBJECT, FIRST_VALUE_OBJECT]);
	object(&[ONE_ASCII_CHARACTER, b'k']);
	for id in FIRST_VALUE_OBJECT..FIRST_VALUE_OBJECT + levels {
		let child = id + 1;
		object(&[TWO_ITEM_ARRAY, child, child]);
	}
	object(&[TRUE]);
	let offset_table = u64::try_from(data.len()).unwrap();
	data.extend_from_slice(&offsets);
	data.extend_from_slice(&[0; TRAILER_RESERVED_BYTES]);
	data.extend_from_slice(&ONE_BYTE_OBJECT_OFFSET_AND_REFERENCE);
	data.extend_from_slice(&u64::try_from(offsets.len()).unwrap().to_be_bytes());
	data.extend_from_slice(&ROOT_OBJECT.to_be_bytes());
	data.extend_from_slice(&offset_table.to_be_bytes());
	data
}
