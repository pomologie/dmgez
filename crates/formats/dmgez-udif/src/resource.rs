#[cfg(feature = "write")]
use plist::Dictionary;
use plist::Value;

use crate::Error;

const RESOURCE_FORK_KEY: &str = "resource-fork";
const BLOCK_MAPS_KEY: &str = "blkx";
pub(crate) const BLOCK_MAP_DATA_KEY: &str = "Data";
pub(crate) const BLOCK_MAP_ID_KEY: &str = "ID";

#[cfg(feature = "write")]
const BLOCK_MAP_ATTRIBUTES_KEY: &str = "Attributes";
#[cfg(feature = "write")]
const BLOCK_MAP_CF_NAME_KEY: &str = "CFName";
#[cfg(feature = "write")]
const BLOCK_MAP_NAME_KEY: &str = "Name";
#[cfg(feature = "write")]
const WHOLE_DISK_ATTRIBUTES: &str = "0x0050";
#[cfg(feature = "write")]
const WHOLE_DISK_ID: &str = "0";
#[cfg(feature = "write")]
const WHOLE_DISK_LABEL: &str = "whole disk (unknown partition : 0)";
#[cfg(feature = "write")]
const PARTITION_LIST_KEY: &str = "plst";
#[cfg(feature = "write")]
const PARTITION_LIST_NAME: &str = "";
#[cfg(feature = "write")]
const NATIVE_PARTITION_LIST_BYTES: usize = 1032;
#[cfg(feature = "write")]
const NATIVE_PARTITION_LIST_MARKERS: [usize; 2] = [517, 519];
#[cfg(feature = "write")]
const NATIVE_PARTITION_LIST_MARKER: u8 = 1;

#[cfg(feature = "read")]
const BINARY_PLIST_PREFIX: &[u8] = b"bplist";
#[cfg(feature = "read")]
const XML_KEY_ELEMENT: &str = "key";
#[cfg(feature = "read")]
const XML_DICTIONARY_ELEMENT: &str = "dict";
#[cfg(feature = "read")]
const XML_INTERNAL_SUBSET_OPEN: char = '[';
#[cfg(feature = "read")]
const XML_MARKUP_OPEN: char = '<';
#[cfg(feature = "read")]
const MAX_DEPTH: usize = 32;
#[cfg(feature = "read")]
const MAX_ELEMENTS: usize = 65_536;

#[cfg(feature = "read")]
pub(crate) fn decode(bytes: &[u8]) -> Result<Value, Error> {
	use std::collections::BTreeSet;

	use quick_xml::{Reader, XmlVersion, escape, events::Event};

	if bytes.starts_with(BINARY_PLIST_PREFIX) {
		return Err(Error::Unsupported("binary resource plist"));
	}
	let mut reader = Reader::from_reader(bytes);
	let mut open = Vec::<Option<BTreeSet<String>>>::new();
	let (mut elements, mut roots, mut doctype) = (0, 0, false);
	loop {
		let event = reader
			.read_event()
			.map_err(|_| Error::Corrupt("malformed resource XML"))?;
		match event {
			Event::Eof => break,
			Event::DocType(ref content) => {
				if doctype || roots != 0 || content.contains(XML_INTERNAL_SUBSET_OPEN) {
					return Err(Error::Unsupported("extended resource XML DTD"));
				}
				doctype = true;
			}
			Event::PI(_) => return Err(Error::Unsupported("resource XML processing instruction")),
			Event::Start(ref tag) | Event::Empty(ref tag) => {
				roots += usize::from(open.is_empty());
				elements += 1;
				if elements > MAX_ELEMENTS || open.len() >= MAX_DEPTH {
					return Err(Error::ResourceLimit("resource XML structure"));
				}
				if tag.name().as_ref() == XML_KEY_ELEMENT {
					let Some(Some(keys)) = open.last_mut() else {
						return Err(Error::Corrupt("property key outside a dictionary"));
					};
					let text = if matches!(event, Event::Start(_)) {
						reader
							.read_text(tag.name())
							.map_err(|_| Error::Corrupt("invalid property key"))?
							.xml_content(XmlVersion::Implicit1_0)
					} else {
						"".into()
					};
					if text.contains(XML_MARKUP_OPEN) {
						return Err(Error::Unsupported("property key with embedded XML"));
					}
					let key = escape::unescape(&text)
						.map_err(|_| Error::Corrupt("invalid property key entity"))?;
					if !keys.insert(key.into_owned()) {
						return Err(Error::Corrupt("duplicate property key"));
					}
				} else if matches!(event, Event::Start(_)) {
					open.push((tag.name().as_ref() == XML_DICTIONARY_ELEMENT).then(BTreeSet::new));
				}
			}
			Event::End(_) => {
				open.pop()
					.ok_or(Error::Corrupt("resource XML element order"))?;
			}
			_ => {}
		}
	}
	if roots != 1 || !open.is_empty() {
		return Err(Error::Corrupt("resource XML must have one complete root"));
	}
	Value::from_reader_xml(bytes).map_err(Error::Resource)
}

#[cfg(feature = "read")]
pub(crate) fn tables(root: &Value) -> Result<&[Value], Error> {
	root.as_dictionary()
		.and_then(|root| root.get(RESOURCE_FORK_KEY))
		.and_then(Value::as_dictionary)
		.and_then(|resources| resources.get(BLOCK_MAPS_KEY))
		.and_then(Value::as_array)
		.filter(|tables| !tables.is_empty())
		.map(Vec::as_slice)
		.ok_or(Error::Corrupt("resource plist is missing block maps"))
}

#[cfg(feature = "write")]
fn block_map_metadata(data: Option<Vec<u8>>) -> Dictionary {
	let mut table = Dictionary::from_iter([
		(BLOCK_MAP_ATTRIBUTES_KEY, Value::from(WHOLE_DISK_ATTRIBUTES)),
		(BLOCK_MAP_CF_NAME_KEY, Value::from(WHOLE_DISK_LABEL)),
	]);
	if let Some(data) = data {
		table.insert(BLOCK_MAP_DATA_KEY.into(), Value::Data(data));
	}
	table.insert(BLOCK_MAP_ID_KEY.into(), WHOLE_DISK_ID.into());
	table.insert(BLOCK_MAP_NAME_KEY.into(), WHOLE_DISK_LABEL.into());
	table
}

#[cfg(feature = "write")]
fn native_partition_list() -> Value {
	let mut payload = vec![0; NATIVE_PARTITION_LIST_BYTES];
	for index in NATIVE_PARTITION_LIST_MARKERS {
		payload[index] = NATIVE_PARTITION_LIST_MARKER;
	}
	let record = Dictionary::from_iter([
		(BLOCK_MAP_ATTRIBUTES_KEY, WHOLE_DISK_ATTRIBUTES.into()),
		(BLOCK_MAP_DATA_KEY, Value::Data(payload)),
		(BLOCK_MAP_ID_KEY, WHOLE_DISK_ID.into()),
		(BLOCK_MAP_NAME_KEY, PARTITION_LIST_NAME.into()),
	]);
	Value::Array(vec![Value::Dictionary(record)])
}

#[cfg(all(feature = "read", feature = "write"))]
pub(crate) fn rebuild_table(root: &Value) -> Result<(&[u8], bool), Error> {
	let unsupported = || Error::Unsupported("resource metadata cannot be preserved by conversion");
	let root_dict = root
		.as_dictionary()
		.filter(|dict| dict.len() == 1)
		.ok_or_else(unsupported)?;
	let resources = root_dict
		.get(RESOURCE_FORK_KEY)
		.and_then(Value::as_dictionary)
		.ok_or_else(unsupported)?;
	let has_partition_list = match resources.get(PARTITION_LIST_KEY) {
		None if resources.len() == 1 => false,
		Some(value) if resources.len() == 2 && *value == native_partition_list() => true,
		_ => return Err(unsupported()),
	};
	let [table] = tables(root)? else {
		return Err(Error::Unsupported(
			"conversion requires one whole-disk block map",
		));
	};
	let table = table.as_dictionary().ok_or_else(unsupported)?;
	let expected = block_map_metadata(None);
	if table.len() != expected.len() + 1
		|| !expected
			.iter()
			.all(|(key, value)| table.get(key) == Some(value))
	{
		return Err(unsupported());
	}
	let data = table
		.get(BLOCK_MAP_DATA_KEY)
		.and_then(Value::as_data)
		.ok_or_else(unsupported)?;
	Ok((data, has_partition_list))
}

#[cfg(feature = "write")]
pub(crate) fn encode(table: Vec<u8>, include_partition_list: bool) -> Result<Vec<u8>, Error> {
	let table_metadata = block_map_metadata(Some(table));
	let mut resources = Dictionary::from_iter([(
		BLOCK_MAPS_KEY,
		Value::Array(vec![Value::Dictionary(table_metadata)]),
	)]);
	if include_partition_list {
		resources.insert(PARTITION_LIST_KEY.into(), native_partition_list());
	}
	let root = Value::Dictionary(Dictionary::from_iter([(
		RESOURCE_FORK_KEY,
		Value::Dictionary(resources),
	)]));
	let mut xml = Vec::new();
	root.to_writer_xml(&mut xml).map_err(Error::Resource)?;
	Ok(xml)
}

#[cfg(all(test, feature = "read"))]
mod tests {
	use crate::Error;

	use super::decode;

	#[test]
	fn rejects_ambiguous_keys_and_entity_declarations() {
		let duplicates = br"<plist><dict><key>Data</key><integer>1</integer><key>D&#97;ta</key><integer>2</integer></dict></plist>";
		assert!(matches!(
			decode(duplicates),
			Err(Error::Corrupt("duplicate property key"))
		));
		let internal_subset =
			br#"<!DOCTYPE plist [<!ENTITY loop "&loop;">]><plist><dict/></plist>"#;
		assert!(matches!(
			decode(internal_subset),
			Err(Error::Unsupported(_))
		));
		assert!(decode(b"<plist><dict><key>Data</key><integer>1</integer></dict></plist>").is_ok());
	}
}
