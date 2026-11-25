//! Rebuild a qualified Finder store after an image catalog edit.

use std::{
	collections::{BTreeMap, HashSet, TryReserveError},
	io::Cursor,
	mem::size_of,
};

use ds_parser::{
	DSStore, FieldKey, FinderViewStyle, FourCC, IconLocation, Record, Value, layout::BTreeMaster,
};
use thiserror::Error;

use crate::{
	inspection::{ReadBudget, ReadError, ReadLimits, RecordValue, ViewKind, visit_records},
	layout::{
		DEFAULT_BACKGROUND_TYPE, SOLID_BACKGROUND_TYPE, VIEW_OPTIONS_VERSION, known_arrangement,
		list::known_column,
	},
	records::{
		CURRENT_FOLDER, LEAF_HEADER_BYTES, LENGTH_BYTES, LOCATION_BYTES, STORE_VERSION,
		record_wire_bytes, validate_item_name,
	},
};

const COORDINATES_BYTES: usize = 2 * size_of::<u32>();
const TRAILING_BYTES: usize = LOCATION_BYTES - COORDINATES_BYTES;
const STANDARD_TRAILING: [u8; TRAILING_BYTES] = IconLocation::new(0, 0).trailing;
const POSITION_ENTRY_BYTES: usize = size_of::<(String, IconLocationRecord)>();

/// One stored `Iloc` record with exact Finder-specific trailing bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IconLocationRecord {
	/// Horizontal coordinate in whole Finder points.
	pub x: u32,
	/// Vertical coordinate in whole Finder points.
	pub y: u32,
	trailing: [u8; TRAILING_BYTES],
}

impl IconLocationRecord {
	/// Creates a record with the writer's standard trailing bytes.
	#[must_use]
	pub const fn new(x: u32, y: u32) -> Self {
		Self {
			x,
			y,
			trailing: STANDARD_TRAILING,
		}
	}

	/// Changes only the coordinates of a retained record.
	#[must_use]
	pub const fn with_position(self, x: u32, y: u32) -> Self {
		Self { x, y, ..self }
	}
}

/// The original record set could not be admitted or safely rebuilt.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreEditError {
	#[error(transparent)]
	Read(#[from] ReadError),
	#[error("Finder field {field:?} for {name:?} is outside the catalog edit profile")]
	UnsupportedRecord { name: String, field: [u8; 4] },
	#[error("Finder store has no qualified window, view, or version record")]
	MissingFolderSettings,
	#[error("Finder store differs from the qualified single-leaf writer framing")]
	UnqualifiedFraming,
	#[error("Finder position name {0:?} is outside the writer's item name profile")]
	InvalidName(String),
	#[error("Finder position name {0:?} collides with another name")]
	DuplicateName(String),
	#[error("edited Finder records exceed the single-leaf budget")]
	LeafBudget,
	#[error("cannot reserve Finder catalog edit memory")]
	Reserve(#[from] TryReserveError),
	#[error("cannot encode the Finder catalog edit")]
	Write(#[from] ds_parser::Error),
}

/// A bounded store whose folder settings can be retained when item names change.
/// The profile uses canonical single-leaf framing, known folder settings without
/// identity references, and ASCII child names for position records.
#[derive(Debug)]
pub struct EditableStore {
	folder: Vec<FolderRecord>,
	positions: Vec<(String, IconLocationRecord)>,
	limits: ReadLimits,
}

impl EditableStore {
	/// Admits a store and retains its folder settings byte for byte.
	/// # Errors
	/// Rejects opaque fields, identity references, unsupported view settings or
	/// icon trailers, malformed or noncanonical framing, and exceeded limits.
	pub fn open(bytes: &[u8], limits: ReadLimits) -> Result<Self, StoreEditError> {
		let mut store = Self {
			folder: Vec::new(),
			positions: Vec::new(),
			limits,
		};
		visit_records(bytes, limits, |record, payload, _| {
			if let RecordValue::IconPosition { x, y } = record.value {
				check_name(&record.name, limits)?;
				let trailing = *payload
					.last_chunk::<TRAILING_BYTES>()
					.ok_or(ReadError::Truncated)?;
				// Only the writer's standard trailer is qualified for rebinding to
				// another catalog entry. In-place edits can keep unknown trailers.
				if trailing != STANDARD_TRAILING {
					return Err(StoreEditError::UnsupportedRecord {
						name: record.name,
						field: record.field,
					});
				}
				store.positions.try_reserve(1)?;
				store
					.positions
					.push((record.name, IconLocationRecord { x, y, trailing }));
			} else {
				store.folder.try_reserve(1)?;
				store.folder.push(FolderRecord::admit(record, payload)?);
			}
			Ok::<(), StoreEditError>(())
		})?;
		for required in [FieldKey::bwsp, FieldKey::icvl, FieldKey::vSrn] {
			if !store.folder.iter().any(|record| record.field == required) {
				return Err(StoreEditError::MissingFolderSettings);
			}
		}
		// Inspection alone does not qualify padding, allocator clients, property
		// list object tables, or record ordering for rewrite. Exact re-encoding
		// checks the original against the same writer used for the result.
		if store.write(store.positions())? != bytes {
			return Err(StoreEditError::UnqualifiedFraming);
		}
		Ok(store)
	}

	/// Exact original item names and their stored icon positions.
	pub fn positions(&self) -> impl ExactSizeIterator<Item = (&str, IconLocationRecord)> {
		self.positions
			.iter()
			.map(|(name, location)| (name.as_str(), *location))
	}

	/// Retained payload, name, and record storage to charge against a capture budget.
	#[must_use]
	pub fn retained_bytes(&self) -> usize {
		size_of::<Self>()
			+ self.folder.capacity() * size_of::<FolderRecord>()
			+ self
				.folder
				.iter()
				.map(|record| record.payload.len())
				.sum::<usize>()
			+ self.positions.capacity() * POSITION_ENTRY_BYTES
			+ self
				.positions
				.iter()
				.map(|(name, _)| name.capacity())
				.sum::<usize>()
	}

	/// Encodes the given final item positions with the original folder settings.
	/// Returns `None` if the source store can be retained byte for byte.
	/// # Errors
	/// Rejects unsupported names, case collisions, and exceeded write limits.
	pub fn encode(
		&self,
		positions: &BTreeMap<String, IconLocationRecord>,
	) -> Result<Option<Vec<u8>>, StoreEditError> {
		if self.positions.len() == positions.len()
			&& self
				.positions()
				.all(|(name, location)| positions.get(name) == Some(&location))
		{
			return Ok(None);
		}
		self.write(
			positions
				.iter()
				.map(|(name, location)| (name.as_str(), *location)),
		)
		.map(Some)
	}

	fn write<'name>(
		&self,
		positions: impl ExactSizeIterator<Item = (&'name str, IconLocationRecord)>,
	) -> Result<Vec<u8>, StoreEditError> {
		let count = self.folder.len().saturating_add(positions.len());
		if count > self.limits.records || count > BTreeMaster::PAGE_SIZE.as_usize() {
			return Err(ReadError::Limit(ReadBudget::Records).into());
		}
		let mut names = HashSet::new();
		names.try_reserve(positions.len())?;
		let mut records = Vec::new();
		records.try_reserve_exact(count)?;
		let mut page_bytes = LEAF_HEADER_BYTES;
		for record in &self.folder {
			page_bytes += record_wire_bytes(CURRENT_FOLDER.len(), record.wire_payload_bytes());
			records.push(record.encode()?);
		}
		if page_bytes > BTreeMaster::PAGE_SIZE.as_usize() {
			return Err(StoreEditError::LeafBudget);
		}
		for (name, location) in positions {
			check_name(name, self.limits)?;
			if !names.insert(name.to_ascii_lowercase()) {
				return Err(StoreEditError::DuplicateName(name.to_owned()));
			}
			page_bytes += record_wire_bytes(name.len(), LENGTH_BYTES + LOCATION_BYTES);
			if page_bytes > BTreeMaster::PAGE_SIZE.as_usize() {
				return Err(StoreEditError::LeafBudget);
			}
			let location = IconLocation::with_trailing(location.x, location.y, location.trailing);
			records.push(Record::new(name, FieldKey::Iloc, location));
		}
		let encoded = DSStore::new(records).to_bytes()?;
		if encoded.len() > self.limits.store_bytes {
			return Err(ReadError::Limit(ReadBudget::StoreBytes).into());
		}
		Ok(encoded)
	}
}

fn check_name(name: &str, limits: ReadLimits) -> Result<(), StoreEditError> {
	if name.len() > limits.name_units || validate_item_name(name).is_err() {
		return Err(StoreEditError::InvalidName(name.to_owned()));
	}
	Ok(())
}

#[derive(Debug)]
struct FolderRecord {
	field: FieldKey,
	payload: Box<[u8]>,
}

impl FolderRecord {
	fn admit(record: crate::inspection::Record, payload: &[u8]) -> Result<Self, StoreEditError> {
		let known = match record.value {
			RecordValue::Window(window) => window.unreported_keys.is_empty(),
			RecordValue::IconView(view) => {
				view.unreported_keys.is_empty()
					&& view.background_alias.is_none()
					&& matches!(
						view.background_type,
						Some(DEFAULT_BACKGROUND_TYPE | SOLID_BACKGROUND_TYPE)
					) && view.arrangement.as_deref().is_some_and(known_arrangement)
					&& view.view_options_version == Some(VIEW_OPTIONS_VERSION)
			}
			RecordValue::ListView(view) => {
				view.unreported_keys.is_empty()
					&& view.view_options_version == Some(VIEW_OPTIONS_VERSION)
					&& view.sort_column.as_deref().is_some_and(known_column)
					&& view.columns.is_some_and(|columns| {
						columns.into_iter().all(|column| {
							column.unreported_keys.is_empty() && known_column(&column.identifier)
						})
					})
			}
			RecordValue::View(ViewKind::Icon) => true,
			RecordValue::View(ViewKind::Other(code)) => {
				[FinderViewStyle::List, FinderViewStyle::Column]
					.into_iter()
					.any(|view| view.fourcc().bytes() == code)
			}
			RecordValue::StoreVersion(version) => version == STORE_VERSION,
			_ => false,
		};
		if !known || record.name != CURRENT_FOLDER {
			return Err(StoreEditError::UnsupportedRecord {
				name: record.name,
				field: record.field,
			});
		}
		Ok(Self {
			field: FieldKey::from_fourcc(FourCC::from(record.field)),
			payload: payload.into(),
		})
	}

	fn wire_payload_bytes(&self) -> usize {
		self.payload.len()
			+ match self.field {
				FieldKey::icvl | FieldKey::vSrn => 0,
				_ => LENGTH_BYTES,
			}
	}

	fn encode(&self) -> Result<Record, ReadError> {
		let value = match self.field {
			FieldKey::icvl => Value::Type(FourCC::from(
				*self.payload.first_chunk().ok_or(ReadError::Truncated)?,
			)),
			FieldKey::vSrn => Value::Long(STORE_VERSION),
			// The bounded inspector has already decoded and qualified this payload.
			_ => Value::PropertyList(plist::Value::from_reader(Cursor::new(&self.payload))?),
		};
		Ok(Record::new(CURRENT_FOLDER, self.field, value))
	}
}
