#![cfg(feature = "read")]

use dmgez_udif::{
	Error, TRAILER_BYTES,
	reader::{Image, Limits, ResourceValue},
};

const NATIVE_UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const NATIVE_RAW: &[u8] = include_bytes!("../../../../tests/fixtures/expected/tiny.raw");
const NATIVE_XML_START: usize = 2048;
const XML_LENGTH_OFFSET: usize = 224;
const BLOCK_MAP_COUNT_OFFSET: usize = 200;
const NATIVE_RECORD_COUNT: u32 = 2;
const RESOURCE_FORK: &str = "resource-fork";
const BLOCK_MAPS: &str = "blkx";
const BLOCK_MAP_DATA: &str = "Data";
const XML_DATA_START: &str = "<data>";
const UNKNOWN_RESOURCE: &str = "UnrecognizedFixtureEntry";
const UNKNOWN_VALUE: &str = "Opaque resource metadata";

fn native_xml() -> &'static [u8] {
	&NATIVE_UDRO[NATIVE_XML_START..NATIVE_UDRO.len() - TRAILER_BYTES]
}

// Replace only the XML on an independent native fixture. The data fork, disk
// geometry and checksums stay fixed, so only plist admission varies.
fn image_with_xml(xml: &[u8]) -> Vec<u8> {
	let mut trailer = *NATIVE_UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	let length = u64::try_from(xml.len()).unwrap().to_be_bytes();
	trailer[XML_LENGTH_OFFSET..XML_LENGTH_OFFSET + length.len()].copy_from_slice(&length);
	[&NATIVE_UDRO[..NATIVE_XML_START], xml, &trailer].concat()
}

fn image_with_resources(resources: &ResourceValue) -> Vec<u8> {
	let mut xml = Vec::new();
	resources.to_writer_xml(&mut xml).unwrap();
	image_with_xml(&xml)
}

#[test]
fn unknown_safe_resource_is_reported_without_changing_the_logical_disk() {
	let mut resources = ResourceValue::from_reader_xml(native_xml()).unwrap();
	resources
		.as_dictionary_mut()
		.unwrap()
		.get_mut(RESOURCE_FORK)
		.unwrap()
		.as_dictionary_mut()
		.unwrap()
		.insert(UNKNOWN_RESOURCE.into(), ResourceValue::from(UNKNOWN_VALUE));
	let bytes = image_with_resources(&resources);
	let mut image = Image::open(bytes.as_slice(), Limits::default()).unwrap();
	let reported = image.descriptor().resources.as_dictionary().unwrap()[RESOURCE_FORK]
		.as_dictionary()
		.unwrap();
	assert_eq!(reported[UNKNOWN_RESOURCE].as_string(), Some(UNKNOWN_VALUE));

	let mut raw = Vec::new();
	image.write_raw_to(&mut raw).unwrap();
	assert_eq!(raw, NATIVE_RAW);
}

#[test]
fn rejects_malformed_base64_in_the_block_map_resource() {
	let xml = std::str::from_utf8(native_xml()).unwrap();
	let (prefix, data) = xml.split_once(XML_DATA_START).unwrap();
	let malformed = format!("{prefix}{XML_DATA_START}!{data}");
	let bytes = image_with_xml(malformed.as_bytes());
	assert!(matches!(
		Image::open(bytes.as_slice(), Limits::default()),
		Err(Error::Resource(_))
	));
}

#[test]
fn rejects_block_map_record_count_that_disagrees_with_decoded_length() {
	let mut resources = ResourceValue::from_reader_xml(native_xml()).unwrap();
	let table = resources
		.as_dictionary_mut()
		.unwrap()
		.get_mut(RESOURCE_FORK)
		.unwrap()
		.as_dictionary_mut()
		.unwrap()
		.get_mut(BLOCK_MAPS)
		.unwrap()
		.as_array_mut()
		.unwrap()[0]
		.as_dictionary_mut()
		.unwrap();
	let ResourceValue::Data(block_map) = table.get_mut(BLOCK_MAP_DATA).unwrap() else {
		panic!("native image has no block-map data");
	};
	let count_range = BLOCK_MAP_COUNT_OFFSET..BLOCK_MAP_COUNT_OFFSET + size_of::<u32>();
	assert_eq!(
		block_map[count_range.clone()],
		NATIVE_RECORD_COUNT.to_be_bytes()
	);
	block_map[count_range].copy_from_slice(&(NATIVE_RECORD_COUNT + 1).to_be_bytes());
	let bytes = image_with_resources(&resources);
	assert!(matches!(
		Image::open(bytes.as_slice(), Limits::default()),
		Err(Error::Corrupt(
			"mish record count does not match its length"
		))
	));
}
