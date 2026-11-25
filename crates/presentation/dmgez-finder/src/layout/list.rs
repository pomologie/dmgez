use std::collections::BTreeSet;

use dmgez_config::{Column, Direction, FinderOptions, ListView};
use ds_parser::{ListViewProperties, PropertyList};
use plist::Dictionary;

use super::{VIEW_OPTIONS_VERSION, point};
use crate::Error;

const COLUMNS: &str = "columns";
const INDEX: &str = "index";
const IDENTIFIER: &str = "identifier";
const WIDTH: &str = "width";
const VISIBLE: &str = "visible";
const ASCENDING: &str = "ascending";
const DEFAULT_ICON_SIZE: f64 = 16.0;
const DEFAULT_TEXT_SIZE: f64 = 12.0;
const DEFAULT_SCROLL_POSITION: (f64, f64) = (0.0, 0.0);
const DEFAULT_VISIBLE: &[Column] = &[
	Column::Name,
	Column::DateModified,
	Column::Size,
	Column::Kind,
	Column::DateAdded,
];

const COLUMN_SPECS: [ColumnSpec; 10] = [
	ColumnSpec::new(Column::Name, "name", 300, Direction::Ascending),
	ColumnSpec::new(
		Column::DateModified,
		"dateModified",
		181,
		Direction::Descending,
	),
	ColumnSpec::new(
		Column::DateCreated,
		"dateCreated",
		181,
		Direction::Descending,
	),
	ColumnSpec::new(Column::DateAdded, "dateAdded", 181, Direction::Descending),
	ColumnSpec::new(
		Column::DateLastOpened,
		"dateLastOpened",
		200,
		Direction::Descending,
	),
	ColumnSpec::new(Column::Size, "size", 97, Direction::Descending),
	ColumnSpec::new(Column::Kind, "kind", 115, Direction::Ascending),
	ColumnSpec::new(Column::Label, "label", 100, Direction::Ascending),
	ColumnSpec::new(Column::Version, "version", 75, Direction::Ascending),
	ColumnSpec::new(Column::Comments, "comments", 300, Direction::Ascending),
];

struct ColumnSpec {
	column: Column,
	identifier: &'static str,
	width: i32,
	direction: Direction,
}

impl ColumnSpec {
	const fn new(
		column: Column,
		identifier: &'static str,
		width: i32,
		direction: Direction,
	) -> Self {
		Self {
			column,
			identifier,
			width,
			direction,
		}
	}
}

fn column_spec(column: Column) -> Result<&'static ColumnSpec, Error> {
	COLUMN_SPECS
		.iter()
		.find(|spec| spec.column == column)
		.ok_or(Error::ListColumn(column))
}

pub(crate) struct ListRecords {
	pub keyed: PropertyList,
	pub ordered: PropertyList,
}

pub(super) fn resolve(options: Option<&FinderOptions>) -> Result<ListRecords, Error> {
	let settings = options.and_then(|settings| settings.list_view.as_ref());
	let sort = column_spec(
		settings
			.and_then(|view| view.sort_by)
			.unwrap_or(Column::Name),
	)?;
	let properties = ListViewProperties {
		calculate_all_sizes: Some(
			settings
				.and_then(|view| view.calculate_all_sizes)
				.unwrap_or(false),
		),
		icon_size: Some(
			settings
				.and_then(|view| view.icon_size)
				.map_or(DEFAULT_ICON_SIZE, |size| size.get()),
		),
		show_icon_preview: Some(
			options
				.and_then(|settings| settings.show_icon_preview)
				.unwrap_or(false),
		),
		scroll_position: Some(
			settings
				.and_then(|view| view.scroll_position)
				.map_or(DEFAULT_SCROLL_POSITION, point),
		),
		sort_column: Some(sort.identifier.to_owned()),
		text_size: Some(
			settings
				.and_then(|view| view.text_size)
				.map_or(DEFAULT_TEXT_SIZE, |size| size.get()),
		),
		use_relative_dates: Some(
			settings
				.and_then(|view| view.use_relative_dates)
				.unwrap_or(true),
		),
		view_options_version: Some(VIEW_OPTIONS_VERSION),
		..ListViewProperties::default()
	};
	let columns = resolve_columns(settings)?;
	let mut ordered = properties
		.to_property_list()
		.into_dictionary()
		.expect("typed Finder list-view properties always serialize to a dictionary");
	let mut keyed = ordered.clone();
	// Finder writes the same settings twice. The lsvp columns are keyed and
	// indexed, while lsvC carries ordered columns with explicit identifiers.
	keyed.insert(COLUMNS.to_owned(), PropertyList::Dictionary(columns.keyed));
	ordered.insert(COLUMNS.to_owned(), PropertyList::Array(columns.ordered));
	Ok(ListRecords {
		keyed: PropertyList::Dictionary(keyed),
		ordered: PropertyList::Dictionary(ordered),
	})
}

#[derive(Default)]
struct ColumnRecords {
	keyed: Dictionary,
	ordered: Vec<PropertyList>,
}

fn resolve_columns(settings: Option<&ListView>) -> Result<ColumnRecords, Error> {
	let visible = settings
		.and_then(|view| view.columns.as_deref())
		.unwrap_or(DEFAULT_VISIBLE);
	let seen = visible.iter().copied().collect::<BTreeSet<_>>();
	if visible.is_empty() || seen.len() != visible.len() {
		return Err(Error::ListColumnOrder);
	}
	if let Some(settings) = settings {
		for &column in settings
			.column_widths
			.keys()
			.chain(settings.sort_directions.keys())
		{
			column_spec(column)?;
		}
	}

	let columns = visible.iter().copied().map(|column| (column, true)).chain(
		COLUMN_SPECS
			.iter()
			.map(|spec| spec.column)
			.filter(|column| !seen.contains(column))
			.map(|column| (column, false)),
	);
	let mut records = ColumnRecords::default();
	for (column, visible) in columns {
		let spec = column_spec(column)?;
		let width = settings
			.and_then(|view| view.column_widths.get(&column))
			.map(|width| width.get());
		let width = width.map_or(Ok(spec.width), |width| column_width(column, width))?;
		let direction = settings
			.and_then(|view| view.sort_directions.get(&column))
			.copied()
			.unwrap_or(spec.direction);
		let ascending = match direction {
			Direction::Ascending => true,
			Direction::Descending => false,
			_ => return Err(Error::ListSortDirection(column)),
		};
		let mut keyed = Dictionary::from_iter([
			(WIDTH.to_owned(), PropertyList::from(i64::from(width))),
			(VISIBLE.to_owned(), PropertyList::Boolean(visible)),
			(ASCENDING.to_owned(), PropertyList::Boolean(ascending)),
		]);
		let mut ordered = keyed.clone();
		let index = u64::try_from(records.ordered.len()).map_err(|_| Error::ListColumnOrder)?;
		keyed.insert(INDEX.to_owned(), PropertyList::from(index));
		ordered.insert(IDENTIFIER.to_owned(), PropertyList::from(spec.identifier));
		records
			.keyed
			.insert(spec.identifier.to_owned(), PropertyList::Dictionary(keyed));
		records.ordered.push(PropertyList::Dictionary(ordered));
	}
	Ok(records)
}

fn column_width(column: Column, width: f64) -> Result<i32, Error> {
	if !(1.0..=f64::from(i32::MAX)).contains(&width) || width.fract() != 0.0 {
		return Err(Error::ListColumnWidth(column));
	}
	Ok(width as i32)
}
