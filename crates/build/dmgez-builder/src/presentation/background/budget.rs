use std::mem::size_of;

use dmgez_config::{
	Background, Column, Coordinate, Dimension, Direction, FinderOptions, VirtualPath,
};
use dmgez_finder::LayoutItem;

use super::{DeferredFinderStore, OwnedItem, source_path_bytes};

const MINIMUM_COLUMN_WIDTH: u8 = 0;

pub(super) fn deferred_metadata_bytes(
	path: &VirtualPath,
	options: &FinderOptions,
	children: &[LayoutItem<'_>],
) -> Option<usize> {
	let mut bytes = size_of::<DeferredFinderStore>().checked_add(path.as_str().len())?;
	match &options.background {
		Some(Background::Image { source, retina }) => {
			bytes = bytes.checked_add(source_path_bytes(source, retina.as_ref())?)?;
		}
		Some(Background::Template { name }) => bytes = bytes.checked_add(name.as_str().len())?,
		_ => {}
	}
	if let Some(icons) = &options.icon_view {
		for name in icons.positions.keys() {
			bytes = bytes
				.checked_add(size_of::<(String, [Coordinate; 2])>())?
				.checked_add(name.len())?;
		}
	}
	if let Some(list) = &options.list_view {
		if let Some(columns) = &list.columns {
			bytes = bytes.checked_add(size_of::<Column>().checked_mul(columns.len())?)?;
		}
		bytes = bytes.checked_add(
			size_of::<(Column, Dimension<MINIMUM_COLUMN_WIDTH>)>()
				.checked_mul(list.column_widths.len())?,
		)?;
		bytes = bytes.checked_add(
			size_of::<(Column, Direction)>().checked_mul(list.sort_directions.len())?,
		)?;
	}
	for item in children {
		bytes = bytes
			.checked_add(size_of::<OwnedItem>() + size_of::<LayoutItem<'_>>())?
			.checked_add(item.name.len())?;
	}
	Some(bytes)
}
