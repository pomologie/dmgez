use std::io::Cursor as IoCursor;

use ds_parser::{BrowserWindowSettings, IconViewProperties};
use plist::Value;

use crate::layout::{BACKGROUND_IMAGE_ALIAS, IMAGE_BACKGROUND_TYPE};

use super::{
	BackgroundAlias, IconView, ReadBudget, ReadError, ReadLimits, Window, WindowBounds, background,
	leaf::Cursor,
};

const PLIST_MAGIC: &[u8; 8] = b"bplist00";
const TRAILER_BYTES: usize = 32;
const TRAILER_PADDING_BYTES: usize = 6;
const INTEGER_WIDTHS: [u8; 5] = [1, 2, 3, 4, 8];
const MARKER_KIND: u8 = 0xf0;
const MARKER_VALUE: u8 = 0x0f;
const INTEGER_MARKER: u8 = 0x10;
const ARRAY_MARKER: u8 = 0xa0;
const ORDERED_SET_MARKER: u8 = 0xb0;
const SET_MARKER: u8 = 0xc0;
const DICTIONARY_MARKER: u8 = 0xd0;
const DICTIONARY_REFS_PER_ENTRY: usize = 2;
const EXTENDED_LENGTH: u8 = MARKER_VALUE;
const MAX_INTEGER_WIDTH_POWER: u8 = 3;
const WINDOW_VIEW_STYLE: &str = "ViewStyle";
const TARGET_URL: &str = "TargetURL";
const TARGET_PATH: &str = "TargetPath";
const SHOW_GRID: &str = "showGrid";
const SORT_BY: &str = "sortBy";

const PREVIEW_PANE_VISIBILITY: &str = "PreviewPaneVisibility";
const SCROLL_POSITION_X: &str = "scrollPositionX";
const SCROLL_POSITION_Y: &str = "scrollPositionY";

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

fn number(value: &Value) -> Option<f64> {
	value
		.as_real()
		.or_else(|| value.as_signed_integer().map(|value| value as f64))
}

fn finite_number(value: Option<f64>) -> Result<Option<f64>, ReadError> {
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
	let trailer_start = blob
		.len()
		.checked_sub(TRAILER_BYTES)
		.ok_or(ReadError::PlistLayout)?;
	let (body, trailer) = blob.split_at(trailer_start);
	if !body.starts_with(PLIST_MAGIC) {
		return Err(ReadError::PlistProfile);
	}
	let mut trailer = Cursor::new(trailer);
	trailer.take(TRAILER_PADDING_BYTES)?;
	let [offset_width, reference_width] = trailer.array()?;
	if !INTEGER_WIDTHS.contains(&offset_width) || !INTEGER_WIDTHS.contains(&reference_width) {
		return Err(ReadError::PlistLayout);
	}
	let count = integer(&trailer.array::<{ size_of::<u64>() }>()?)?;
	let root = integer(&trailer.array::<{ size_of::<u64>() }>()?)?;
	let table_offset = integer(&trailer.array::<{ size_of::<u64>() }>()?)?;
	if count > limits.plist_objects {
		return Err(ReadError::Limit(ReadBudget::PlistObjects));
	}
	if root >= count || !(PLIST_MAGIC.len()..=body.len()).contains(&table_offset) {
		return Err(ReadError::PlistLayout);
	}
	let (objects, offsets) = body.split_at(table_offset);
	let offset_width = usize::from(offset_width);
	let table_bytes = count
		.checked_mul(offset_width)
		.ok_or(ReadError::PlistLayout)?;
	let addresses = offsets.get(..table_bytes).ok_or(ReadError::PlistLayout)?;
	let mut entries = None;
	for (index, address) in addresses.chunks_exact(offset_width).enumerate() {
		let offset = integer(address)?;
		if offset < PLIST_MAGIC.len() {
			return Err(ReadError::PlistLayout);
		}
		let mut object = Cursor::new(objects.get(offset..).ok_or(ReadError::PlistLayout)?);
		let [marker] = object.array()?;
		if index == root {
			if marker & MARKER_KIND != DICTIONARY_MARKER {
				return Err(ReadError::PlistProfile);
			}
			let count = object_length(marker, &mut object)?;
			if count > limits.plist_entries {
				return Err(ReadError::Limit(ReadBudget::PlistEntries));
			}
			let bytes_per_entry = DICTIONARY_REFS_PER_ENTRY * usize::from(reference_width);
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

fn object_length(marker: u8, input: &mut Cursor<'_>) -> Result<usize, ReadError> {
	let compact = marker & MARKER_VALUE;
	if compact != EXTENDED_LENGTH {
		return Ok(usize::from(compact));
	}
	let [marker] = input.array()?;
	let power = marker & MARKER_VALUE;
	if marker & MARKER_KIND != INTEGER_MARKER || power > MAX_INTEGER_WIDTH_POWER {
		return Err(ReadError::PlistLayout);
	}
	integer(input.take(1 << power)?)
}

fn integer(bytes: &[u8]) -> Result<usize, ReadError> {
	let mut value = [0; size_of::<u64>()];
	let start = value
		.len()
		.checked_sub(bytes.len())
		.ok_or(ReadError::PlistLayout)?;
	value[start..].copy_from_slice(bytes);
	u64::from_be_bytes(value)
		.try_into()
		.map_err(|_| ReadError::PlistLayout)
}
