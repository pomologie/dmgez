use std::collections::HashSet;

use ds_parser::FieldKey;
use plist::{Dictionary, Value};

use super::{ReadError, ReadLimits, list_plist, properties};

#[cfg(test)]
mod tests;

const ASCENDING: &str = "ascending";
const CALCULATE_ALL_SIZES: &str = "calculateAllSizes";
const COLUMNS: &str = "columns";
const ICON_SIZE: &str = "iconSize";
const IDENTIFIER: &str = "identifier";
const INDEX: &str = "index";
const SHOW_ICON_PREVIEW: &str = "showIconPreview";
const SORT_COLUMN: &str = "sortColumn";
const TEXT_SIZE: &str = "textSize";
const USE_RELATIVE_DATES: &str = "useRelativeDates";
const VIEW_OPTIONS_VERSION: &str = "viewOptionsVersion";
const VISIBLE: &str = "visible";
const WIDTH: &str = "width";
const EXACT_INTEGER_LIMIT: u64 = 1 << f64::MANTISSA_DIGITS;

/// The supported settings from a single Finder list-view record.
#[derive(Debug, PartialEq)]
pub struct ListView {
	pub calculate_all_sizes: Option<bool>,
	pub icon_size: Option<f64>,
	pub scroll_position_x: Option<f64>,
	pub scroll_position_y: Option<f64>,
	pub show_icon_preview: Option<bool>,
	pub sort_column: Option<String>,
	pub text_size: Option<f64>,
	pub use_relative_dates: Option<bool>,
	pub view_options_version: Option<i64>,
	/// Stored column order. Keyed columns are ordered by their explicit index.
	pub columns: Option<Vec<ListColumn>>,
	/// Sorted names of unreported settings. Their values are omitted.
	pub unreported_keys: Vec<String>,
}

/// A Finder list column, including columns outside the writer's vocabulary.
#[derive(Debug, PartialEq, Eq)]
pub struct ListColumn {
	pub identifier: String,
	pub index: u64,
	pub width: u64,
	pub visible: bool,
	pub ascending: bool,
	/// Sorted names of unreported column settings. Their values are omitted.
	pub unreported_keys: Vec<String>,
}

pub(super) fn read(
	blob: &[u8],
	limits: ReadLimits,
	field: FieldKey,
) -> Result<ListView, ReadError> {
	let mut dictionary = list_plist::read(blob, limits)?;
	let columns = dictionary
		.remove(COLUMNS)
		.map(|value| columns(value, field))
		.transpose()?;
	Ok(ListView {
		calculate_all_sizes: setting(&mut dictionary, CALCULATE_ALL_SIZES, Value::as_boolean)?,
		icon_size: real(&mut dictionary, ICON_SIZE)?,
		scroll_position_x: real(&mut dictionary, properties::SCROLL_POSITION_X)?,
		scroll_position_y: real(&mut dictionary, properties::SCROLL_POSITION_Y)?,
		show_icon_preview: setting(&mut dictionary, SHOW_ICON_PREVIEW, Value::as_boolean)?,
		sort_column: dictionary
			.remove(SORT_COLUMN)
			.map(|value| value.into_string().ok_or(ReadError::PlistProfile))
			.transpose()?,
		text_size: real(&mut dictionary, TEXT_SIZE)?,
		use_relative_dates: setting(&mut dictionary, USE_RELATIVE_DATES, Value::as_boolean)?,
		view_options_version: setting(
			&mut dictionary,
			VIEW_OPTIONS_VERSION,
			Value::as_signed_integer,
		)?,
		columns,
		unreported_keys: unreported(dictionary),
	})
}

fn columns(value: Value, field: FieldKey) -> Result<Vec<ListColumn>, ReadError> {
	let mut columns = match (field, value) {
		(FieldKey::lsvp, Value::Dictionary(keyed)) => keyed
			.into_iter()
			.map(|(identifier, value)| {
				let mut dictionary = dictionary(value)?;
				let index = required(&mut dictionary, INDEX, Value::as_unsigned_integer)?;
				column(identifier, index, dictionary)
			})
			.collect::<Result<Vec<_>, _>>()?,
		(FieldKey::lsvC, Value::Array(ordered)) => ordered
			.into_iter()
			.zip(0..)
			.map(|(value, index)| {
				let mut dictionary = dictionary(value)?;
				let identifier = dictionary
					.remove(IDENTIFIER)
					.and_then(Value::into_string)
					.ok_or(ReadError::PlistProfile)?;
				column(identifier, index, dictionary)
			})
			.collect::<Result<Vec<_>, _>>()?,
		_ => return Err(ReadError::PlistProfile),
	};

	let mut identifiers = HashSet::new();
	let mut indices = HashSet::new();
	identifiers.try_reserve(columns.len())?;
	indices.try_reserve(columns.len())?;
	for column in &columns {
		if !identifiers.insert(column.identifier.as_str()) || !indices.insert(column.index) {
			return Err(ReadError::PlistProfile);
		}
	}
	columns.sort_unstable_by_key(|column| column.index);
	Ok(columns)
}

fn column(
	identifier: String,
	index: u64,
	mut dictionary: Dictionary,
) -> Result<ListColumn, ReadError> {
	if identifier.is_empty() {
		return Err(ReadError::PlistProfile);
	}
	Ok(ListColumn {
		identifier,
		index,
		width: required(&mut dictionary, WIDTH, Value::as_unsigned_integer)?,
		visible: required(&mut dictionary, VISIBLE, Value::as_boolean)?,
		ascending: required(&mut dictionary, ASCENDING, Value::as_boolean)?,
		unreported_keys: unreported(dictionary),
	})
}

fn dictionary(value: Value) -> Result<Dictionary, ReadError> {
	value.into_dictionary().ok_or(ReadError::PlistProfile)
}

fn setting<T>(
	dictionary: &mut Dictionary,
	key: &str,
	parse: impl FnOnce(&Value) -> Option<T>,
) -> Result<Option<T>, ReadError> {
	dictionary
		.remove(key)
		.map(|value| parse(&value).ok_or(ReadError::PlistProfile))
		.transpose()
}

fn required<T>(
	dictionary: &mut Dictionary,
	key: &str,
	parse: impl FnOnce(&Value) -> Option<T>,
) -> Result<T, ReadError> {
	setting(dictionary, key, parse)?.ok_or(ReadError::PlistProfile)
}

fn real(dictionary: &mut Dictionary, key: &str) -> Result<Option<f64>, ReadError> {
	let number = setting(dictionary, key, |value| match value.as_signed_integer() {
		Some(integer) if integer.unsigned_abs() > EXACT_INTEGER_LIMIT => None,
		_ => properties::number(value),
	})?;
	properties::finite_number(number)
}

fn unreported(dictionary: Dictionary) -> Vec<String> {
	let mut keys = dictionary
		.into_iter()
		.map(|(key, _)| key)
		.collect::<Vec<_>>();
	keys.sort_unstable();
	keys
}
