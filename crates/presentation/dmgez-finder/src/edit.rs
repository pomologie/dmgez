//! Lossless edits of existing icon positions in bounded Finder stores.

use std::collections::HashMap;

use thiserror::Error;

use crate::inspection::{ReadError, ReadLimits, RecordValue, visit_records};

const COORDINATE_BYTES: usize = size_of::<u32>();

/// Replacement coordinates for one existing icon location in the store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IconPositionEdit<'a> {
	/// Exact decoded name of the existing record.
	pub name: &'a str,
	/// Icon center in whole Finder points from the left of the content area.
	pub x: u32,
	/// Icon center in whole Finder points from the top of the content area.
	pub y: u32,
}

/// A rejected Finder store edit. The input bytes remain intact.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EditError {
	#[error(transparent)]
	Read(#[from] ReadError),
	#[error("Finder icon edits exceed the record budget")]
	TooManyEdits,
	#[error("Finder icon edit name is empty or exceeds the name budget")]
	InvalidName,
	#[error("multiple Finder icon edits target {0:?}")]
	DuplicateTarget(String),
	#[error("no existing Finder icon position for {0:?}")]
	MissingPosition(String),
	#[error("cannot reserve Finder edit memory")]
	Reserve(#[from] std::collections::TryReserveError),
}

/// Replace existing `Iloc` coordinates within the admitted single-leaf profile.
///
/// Only the two coordinate words in each selected record change. Unknown
/// records with supported wire types, reserved payload bytes and store
/// allocation bytes are all retained. Name matching is exact.
///
/// # Errors
///
/// Returns an error for any unsupported or malformed record, budget overrun,
/// missing icon position, or repeated target. The entire batch is validated
/// before any bytes change.
pub fn edit_icon_positions(
	store: &mut [u8],
	edits: &[IconPositionEdit<'_>],
	limits: ReadLimits,
) -> Result<(), EditError> {
	if edits.len() > limits.records {
		return Err(EditError::TooManyEdits);
	}
	let mut pending = HashMap::new();
	pending.try_reserve(edits.len())?;
	for edit in edits {
		if edit.name.is_empty() || edit.name.encode_utf16().nth(limits.name_units).is_some() {
			return Err(EditError::InvalidName);
		}
		if pending.insert(edit.name, (edit.x, edit.y)).is_some() {
			return Err(EditError::DuplicateTarget(edit.name.to_owned()));
		}
	}

	let mut replacements = Vec::new();
	replacements.try_reserve_exact(edits.len())?;
	visit_records(store, limits, |record, offset| {
		if matches!(record.value, RecordValue::IconPosition { .. })
			&& let Some(coordinates) = pending.remove(record.name.as_str())
		{
			replacements.push((offset, coordinates));
		}
	})?;
	if let Some(edit) = edits.iter().find(|edit| pending.contains_key(edit.name)) {
		return Err(EditError::MissingPosition(edit.name.to_owned()));
	}

	for (offset, (x, y)) in replacements {
		let y_offset = offset + COORDINATE_BYTES;
		store[offset..y_offset].copy_from_slice(&x.to_be_bytes());
		store[y_offset..y_offset + COORDINATE_BYTES].copy_from_slice(&y.to_be_bytes());
	}
	Ok(())
}
