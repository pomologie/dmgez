use std::{
	collections::{BTreeMap, BTreeSet},
	mem::size_of,
};

use dmgez_config::Coordinate;
use ds_parser::{DSStore, FieldKey, IconLocation, PropertyList, Record, layout::BTreeMaster};

use crate::{CatalogNames, Error, LayoutItem, layout::Layout};

pub(super) const CURRENT_FOLDER: &str = ".";
const PARENT_FOLDER: &str = "..";
pub(super) const STORE_VERSION: u32 = 1;
const MAX_CATALOG_NAME_UNITS: usize = 255;
const NAME_SEPARATORS: [char; 2] = ['/', ':'];
const UTF16_UNIT_BYTES: usize = size_of::<u16>();
pub(super) const LENGTH_BYTES: usize = size_of::<u32>();
const FOUR_CC_BYTES: usize = size_of::<[u8; 4]>();
const RECORD_HEADER_BYTES: usize = LENGTH_BYTES + 2 * FOUR_CC_BYTES;
const SCALAR_BYTES: usize = size_of::<u32>();
const LOCATION_TRAILING_BYTES: usize = size_of::<[u8; 8]>();
pub(super) const LOCATION_BYTES: usize = 2 * SCALAR_BYTES + LOCATION_TRAILING_BYTES;
pub(super) const LEAF_HEADER_BYTES: usize = 2 * SCALAR_BYTES;

pub(super) struct Store {
	records: Vec<Record>,
	used_bytes: usize,
}

impl Store {
	pub fn for_layout(layout: Layout<'_>) -> Result<Self, Error> {
		let mut store = Self {
			records: Vec::new(),
			used_bytes: LEAF_HEADER_BYTES,
		};
		let window_bytes = plist_payload_size(&layout.window.to_property_list())?;
		store.push(CURRENT_FOLDER, FieldKey::bwsp, layout.window, window_bytes)?;
		store.push(
			CURRENT_FOLDER,
			FieldKey::icvl,
			layout.view.fourcc(),
			FOUR_CC_BYTES,
		)?;
		if let Some(icons) = layout.icons {
			let bytes = plist_payload_size(&icons.to_property_list())?;
			store.push(CURRENT_FOLDER, FieldKey::icvp, icons, bytes)?;
		}
		if let Some(list) = layout.list {
			let keyed_bytes = plist_payload_size(&list.keyed)?;
			store.push(CURRENT_FOLDER, FieldKey::lsvp, list.keyed, keyed_bytes)?;
			let ordered_bytes = plist_payload_size(&list.ordered)?;
			store.push(CURRENT_FOLDER, FieldKey::lsvC, list.ordered, ordered_bytes)?;
		}
		store.push(CURRENT_FOLDER, FieldKey::vSrn, STORE_VERSION, SCALAR_BYTES)?;
		Ok(store)
	}

	pub fn position_items(
		&mut self,
		items: &[LayoutItem<'_>],
		configured: Option<&BTreeMap<String, [Coordinate; 2]>>,
		names: CatalogNames,
	) -> Result<(), Error> {
		if let Some(configured) = configured {
			for name in configured.keys() {
				validate_item_name(name)?;
			}
		}
		let mut matched = BTreeSet::new();
		let mut positioned = BTreeSet::new();
		for item in items {
			let folder_position =
				configured.and_then(|positions| positions.get_key_value(item.name));
			let position = match (item.position, folder_position) {
				(Some(_), Some(_)) => return Err(Error::DuplicatePosition(item.name.to_owned())),
				(Some(position), None) => position,
				(None, Some((name, position))) => {
					matched.insert(name);
					*position
				}
				(None, None) => continue,
			};
			let name = names.stored(item.name)?;
			validate_item_name(&name)?;
			// Match the writer's case-insensitive key ordering.
			if !positioned.insert(name.to_lowercase()) {
				return Err(Error::DuplicateName(item.name.to_owned()));
			}
			let [x, y] = position.map(Coordinate::get);
			let location = match (integer_point(x), integer_point(y)) {
				(Some(x), Some(y)) => IconLocation::new(x, y),
				_ => return Err(Error::Position(item.name.to_owned())),
			};
			self.push(
				&name,
				FieldKey::Iloc,
				location,
				LENGTH_BYTES + LOCATION_BYTES,
			)?;
		}
		if let Some(configured) = configured
			&& let Some(name) = configured.keys().find(|name| !matched.contains(name))
		{
			return Err(Error::MissingItem(name.clone()));
		}
		Ok(())
	}

	pub fn finish(self) -> Result<Vec<u8>, Error> {
		Ok(DSStore::new(self.records).to_bytes()?)
	}

	fn push(
		&mut self,
		name: &str,
		field: FieldKey,
		value: impl Into<ds_parser::Value>,
		payload_bytes: usize,
	) -> Result<(), Error> {
		// Plist values are measured with the same plist serializer the dependency
		// writes. Exclude the unqualified multi-leaf writer.
		let record_bytes = record_wire_bytes(name.encode_utf16().count(), payload_bytes);
		self.used_bytes += record_bytes;
		if self.used_bytes > BTreeMaster::PAGE_SIZE.as_usize() {
			return Err(Error::StoreBudget);
		}
		self.records.push(Record::new(name, field, value));
		Ok(())
	}
}

fn plist_payload_size(plist: &PropertyList) -> Result<usize, Error> {
	let mut bytes = Vec::new();
	plist.to_writer_binary(&mut bytes)?;
	Ok(LENGTH_BYTES + bytes.len())
}

pub(super) const fn record_wire_bytes(name_units: usize, payload_bytes: usize) -> usize {
	RECORD_HEADER_BYTES + name_units * UTF16_UNIT_BYTES + payload_bytes
}

pub(super) fn validate_item_name(name: &str) -> Result<(), Error> {
	if name.is_empty()
		|| name.encode_utf16().take(MAX_CATALOG_NAME_UNITS + 1).count() > MAX_CATALOG_NAME_UNITS
		|| name == CURRENT_FOLDER
		|| name == PARENT_FOLDER
		|| name.contains(NAME_SEPARATORS)
		|| name.chars().any(char::is_control)
	{
		return Err(Error::ItemName);
	}
	Ok(())
}

fn integer_point(point: f64) -> Option<u32> {
	((0.0..=f64::from(u32::MAX)).contains(&point) && point.fract() == 0.0).then_some(point as u32)
}
