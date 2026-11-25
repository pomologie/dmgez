use std::borrow::Cow;

use dmgez_config::{AutoFlag, FinderOptions, ListView};

pub(crate) fn without_positions(options: &FinderOptions) -> FinderOptions {
	let mut style = options.clone();
	if let Some(icons) = &mut style.icon_view {
		icons.positions.clear();
	}
	style
}

pub(crate) fn resolve_options<'a>(
	inherited: Option<&'a FinderOptions>,
	overrides: Option<&'a FinderOptions>,
) -> Option<Cow<'a, FinderOptions>> {
	let (base, local) = match (inherited, overrides) {
		(Some(base), Some(local)) => (base, local),
		(base, local) => return local.or(base).map(Cow::Borrowed),
	};
	let mut options = base.clone();
	// A local opt-out suppresses inherited secondary-view preferences. An
	// explicit local preference still has to pass the writer's view policy.
	if local.include_icon_view_settings == Some(AutoFlag::Disabled) {
		options.background = None;
		options.icon_view = None;
		options.show_item_info = None;
	}
	if local.include_list_view_settings == Some(AutoFlag::Disabled) {
		options.list_view = None;
	}
	options.background = local.background.clone().or(options.background);
	options.window_position = local.window_position.or(options.window_position);
	options.window_size = local.window_size.or(options.window_size);
	options.default_view = local.default_view.or(options.default_view);
	options.sidebar_width = local.sidebar_width.or(options.sidebar_width);
	options.include_icon_view_settings = local
		.include_icon_view_settings
		.or(options.include_icon_view_settings);
	options.include_list_view_settings = local
		.include_list_view_settings
		.or(options.include_list_view_settings);
	options.show_status_bar = local.show_status_bar.or(options.show_status_bar);
	options.show_tab_view = local.show_tab_view.or(options.show_tab_view);
	options.show_toolbar = local.show_toolbar.or(options.show_toolbar);
	options.show_pathbar = local.show_pathbar.or(options.show_pathbar);
	options.show_sidebar = local.show_sidebar.or(options.show_sidebar);
	options.show_icon_preview = local.show_icon_preview.or(options.show_icon_preview);
	options.show_item_info = local.show_item_info.or(options.show_item_info);
	if let Some(local_icons) = &local.icon_view {
		let mut icons = local_icons.clone();
		if let Some(base_icons) = &options.icon_view {
			icons.arrange_by = icons.arrange_by.or(base_icons.arrange_by);
			icons.grid_offset = icons.grid_offset.or(base_icons.grid_offset);
			icons.grid_spacing = icons.grid_spacing.or(base_icons.grid_spacing);
			icons.scroll_position = icons.scroll_position.or(base_icons.scroll_position);
			icons.label_position = icons.label_position.or(base_icons.label_position);
			icons.text_size = icons.text_size.or(base_icons.text_size);
			icons.icon_size = icons.icon_size.or(base_icons.icon_size);
		}
		options.icon_view = Some(icons);
	}
	if let Some(local_list) = &local.list_view {
		if let Some(list) = &mut options.list_view {
			apply_list_overrides(list, local_list);
		} else {
			options.list_view = Some(local_list.clone());
		}
	}
	Some(Cow::Owned(options))
}

fn apply_list_overrides(list: &mut ListView, local: &ListView) {
	list.icon_size = local.icon_size.or(list.icon_size);
	list.text_size = local.text_size.or(list.text_size);
	list.scroll_position = local.scroll_position.or(list.scroll_position);
	list.sort_by = local.sort_by.or(list.sort_by);
	list.use_relative_dates = local.use_relative_dates.or(list.use_relative_dates);
	list.calculate_all_sizes = local.calculate_all_sizes.or(list.calculate_all_sizes);
	if let Some(columns) = &local.columns {
		list.columns = Some(columns.clone());
	}
	list.column_widths.extend(&local.column_widths);
	list.sort_directions.extend(&local.sort_directions);
}
