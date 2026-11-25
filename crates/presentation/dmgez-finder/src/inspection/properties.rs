use std::io::Cursor as IoCursor;

use ds_parser::{BrowserWindowSettings, IconViewProperties};
use plist::Value;

use crate::layout::{BACKGROUND_IMAGE_ALIAS, IMAGE_BACKGROUND_TYPE};

use super::{
	BackgroundAlias, IconView, ReadBudget, ReadError, ReadLimits, Window, WindowBounds, background,
	binary_plist::{
		ARRAY_MARKER, DICTIONARY_MARKER, DICTIONARY_REFS_PER_ENTRY, MARKER_KIND,
		ORDERED_SET_MARKER, OffsetTable, PLIST_MAGIC, SET_MARKER, object_length,
	},
	leaf::Cursor,
};

const WINDOW_VIEW_STYLE: &str = "ViewStyle";
const TARGET_URL: &str = "TargetURL";
const TARGET_PATH: &str = "TargetPath";
const SHOW_GRID: &str = "showGrid";
const SORT_BY: &str = "sortBy";

const PREVIEW_PANE_VISIBILITY: &str = "PreviewPaneVisibility";
pub(super) const SCROLL_POSITION_X: &str = "scrollPositionX";
pub(super) const SCROLL_POSITION_Y: &str = "scrollPositionY";

pub(super) fn window(blob: &[u8], limits: ReadLimits) -> Result<Window, ReadError> {
	let dictionary = read_dictionary(blob, limits)?;
	let mut settings = BrowserWindowSettings::from_property_list(dictionary)
		.map_err(|_| ReadError::PlistProfile)?;
	let bounds = settings.window_bounds.map(|bounds| WindowBounds {
		x: bounds.x,
		y: bounds.y,
		width: bounds.width,
		height: bounds.height,
	});
	if let Some(bounds) = bounds {
		finite(Some([bounds.x, bounds.y, bounds.width, bounds.height]))?;
	}
	let preview_pane_visible = settings
		.unknown
		.get(PREVIEW_PANE_VISIBILITY)
		.and_then(Value::as_boolean);
	if preview_pane_visible.is_some() {
		settings.unknown.remove(PREVIEW_PANE_VISIBILITY);
	}
	let mut unreported_keys = settings.unknown.into_keys().collect::<Vec<_>>();
	append_unreported(
		&mut unreported_keys,
		[
			(settings.view_style.is_some(), WINDOW_VIEW_STYLE),
			(settings.target_url.is_some(), TARGET_URL),
			(settings.target_path.is_some(), TARGET_PATH),
		],
	);
	Ok(Window {
		bounds,
		show_sidebar: settings.show_sidebar,
		show_toolbar: settings.show_toolbar,
		show_status_bar: settings.show_status_bar,
		show_path_bar: settings.show_path_bar,
		show_tab_view: settings.show_tab_view,
		container_show_sidebar: settings.container_show_sidebar,
		preview_pane_visible,
		sidebar_width: finite_number(settings.sidebar_width)?,
		unreported_keys,
	})
}

pub(super) fn icons(blob: &[u8], limits: ReadLimits) -> Result<IconView, ReadError> {
	let dictionary = read_dictionary(blob, limits)?;
	let mut settings =
		IconViewProperties::from_property_list(dictionary).map_err(|_| ReadError::PlistProfile)?;
	let background_alias = match settings.unknown.remove(BACKGROUND_IMAGE_ALIAS) {
		Some(Value::Data(bytes)) => Some(background::inspect(&bytes)),
		Some(_) => Some(BackgroundAlias::Malformed),
		None if settings.background_type == Some(IMAGE_BACKGROUND_TYPE) => {
			Some(BackgroundAlias::Missing)
		}
		None => None,
	};
	let scroll_position = match (
		settings.unknown.get(SCROLL_POSITION_X).and_then(number),
		settings.unknown.get(SCROLL_POSITION_Y).and_then(number),
	) {
		(Some(x), Some(y)) => {
			settings.unknown.remove(SCROLL_POSITION_X);
			settings.unknown.remove(SCROLL_POSITION_Y);
			finite(Some([x, y]))?
		}
		_ => None,
	};
	let mut unreported_keys = settings.unknown.into_keys().collect::<Vec<_>>();
	append_unreported(
		&mut unreported_keys,
		[
			(settings.show_grid.is_some(), SHOW_GRID),
			(settings.sort_by.is_some(), SORT_BY),
		],
	);
	Ok(IconView {
		arrangement: settings.arrange_by,
		background_type: settings.background_type,
		background_color: finite(settings.background_color.map(|(r, g, b)| [r, g, b]))?,
		background_alias,
		grid_offset: finite(settings.grid_offset.map(|(x, y)| [x, y]))?,
		grid_spacing: finite_number(settings.grid_spacing)?,
		scroll_position,
		icon_size: finite_number(settings.icon_size)?,
		text_size: finite_number(settings.text_size)?,
		label_on_bottom: settings.label_on_bottom,
		show_icon_preview: settings.show_icon_preview,
		show_item_info: settings.show_item_info,
		view_options_version: settings.view_options_version,
		unreported_keys,
	})
}

fn append_unreported(
	keys: &mut Vec<String>,
	extra: impl IntoIterator<Item = (bool, &'static str)>,
) {
	keys.extend(
		extra
			.into_iter()
			.filter(|(present, _)| *present)
			.map(|(_, key)| key.to_owned()),
	);
	keys.sort_unstable();
}

pub(super) fn number(value: &Value) -> Option<f64> {
	value
		.as_real()
		.or_else(|| value.as_signed_integer().map(|value| value as f64))
}

pub(super) fn finite_number(value: Option<f64>) -> Result<Option<f64>, ReadError> {
	finite(value.map(|number| [number])).map(|value| value.map(|[number]| number))
}

fn finite<const N: usize>(numbers: Option<[f64; N]>) -> Result<Option<[f64; N]>, ReadError> {
	if numbers.is_some_and(|numbers| !numbers.into_iter().all(f64::is_finite)) {
		return Err(ReadError::SettingsNumber);
	}
	Ok(numbers)
}

fn read_dictionary(blob: &[u8], limits: ReadLimits) -> Result<Value, ReadError> {
	let entries = admit_dictionary(blob, limits)?;
	let value = Value::from_reader(IoCursor::new(blob))?;
	if value
		.as_dictionary()
		.is_none_or(|dictionary| dictionary.len() != entries)
	{
		return Err(ReadError::PlistLayout);
	}
	Ok(value)
}

// Admit only a flat dictionary and bound its references before asking the
// general plist decoder to materialize it. A shared nested object graph can
// otherwise expand beyond the size of the input page.
fn admit_dictionary(blob: &[u8], limits: ReadLimits) -> Result<usize, ReadError> {
	let table = OffsetTable::read(blob, limits)?;
	let mut entries = None;
	for (index, offset) in table.addresses().enumerate() {
		let offset = offset?;
		if offset < PLIST_MAGIC.len() {
			return Err(ReadError::PlistLayout);
		}
		let mut object = Cursor::new(table.objects.get(offset..).ok_or(ReadError::PlistLayout)?);
		let [marker] = object.array()?;
		if index == table.root {
			if marker & MARKER_KIND != DICTIONARY_MARKER {
				return Err(ReadError::PlistProfile);
			}
			let count = object_length(marker, &mut object)?;
			if count > limits.plist_entries {
				return Err(ReadError::Limit(ReadBudget::PlistEntries));
			}
			let bytes_per_entry = DICTIONARY_REFS_PER_ENTRY * table.reference_width;
			if count > object.remaining().len() / bytes_per_entry {
				return Err(ReadError::PlistLayout);
			}
			entries = Some(count);
		} else if matches!(
			marker & MARKER_KIND,
			ARRAY_MARKER | ORDERED_SET_MARKER | SET_MARKER | DICTIONARY_MARKER
		) {
			return Err(ReadError::PlistProfile);
		}
	}
	entries.ok_or(ReadError::PlistLayout)
}
