use std::collections::BTreeMap;

use dmgez_config::{
	Arrangement, AutoFlag, Background, BackgroundTemplate, Column, Coordinate, FinderOptions,
	LabelPosition, View, parse_solid_background_color,
};
use ds_parser::{
	BrowserWindowSettings, FinderViewStyle, IconViewProperties, PropertyList, WindowBounds,
};

use crate::Error;

pub(super) mod list;

const DEFAULT_WINDOW_POSITION: (f64, f64) = (100.0, 100.0);
const DEFAULT_WINDOW_SIZE: (f64, f64) = (640.0, 280.0);
const DEFAULT_SIDEBAR_WIDTH: f64 = 180.0;
const DEFAULT_GRID_OFFSET: (f64, f64) = (0.0, 0.0);
const DEFAULT_SCROLL_POSITION: (f64, f64) = (0.0, 0.0);
const DEFAULT_GRID_SPACING: f64 = 80.0;
const DEFAULT_ICON_SIZE: f64 = 128.0;
const DEFAULT_TEXT_SIZE: f64 = 16.0;
const DEFAULT_BACKGROUND_RGB: (f64, f64, f64) = (1.0, 1.0, 1.0);
const DEFAULT_BACKGROUND_TYPE: i64 = 0;
const SOLID_BACKGROUND_TYPE: i64 = 1;
pub(super) const IMAGE_BACKGROUND_TYPE: i64 = 2;
pub(super) const BACKGROUND_IMAGE_ALIAS: &str = "backgroundImageAlias";
const VIEW_OPTIONS_VERSION: i64 = 1;
const PREVIEW_PANE_VISIBILITY: &str = "PreviewPaneVisibility";
const SCROLL_POSITION_X: &str = "scrollPositionX";
const SCROLL_POSITION_Y: &str = "scrollPositionY";
const MANUAL_ARRANGEMENT: &str = "none";
const ARRANGE_BY_NAME: &str = "name";
const ARRANGE_BY_DATE_MODIFIED: &str = "dateModified";
const ARRANGE_BY_DATE_CREATED: &str = "dateCreated";
const ARRANGE_BY_DATE_ADDED: &str = "dateAdded";
const ARRANGE_BY_DATE_LAST_OPENED: &str = "dateLastOpened";
const ARRANGE_BY_SIZE: &str = "size";
const ARRANGE_BY_KIND: &str = "kind";
const ARRANGE_BY_LABEL: &str = "label";

pub(super) struct Layout<'a> {
	pub window: BrowserWindowSettings,
	pub view: FinderViewStyle,
	pub icons: Option<IconViewProperties>,
	pub list: Option<list::ListRecords>,
	pub positions: Option<&'a BTreeMap<String, [Coordinate; 2]>>,
}

pub(super) enum ImageBackground {
	Unavailable,
	Preflight,
	Alias(Vec<u8>),
}

impl<'a> Layout<'a> {
	pub fn resolve(
		options: Option<&'a FinderOptions>,
		image: ImageBackground,
	) -> Result<Self, Error> {
		let view = match options.and_then(|options| options.default_view) {
			None | Some(View::IconView) => FinderViewStyle::Icon,
			Some(View::ListView) => FinderViewStyle::List,
			Some(view) => return Err(Error::View(view)),
		};
		let icon_switch = options.and_then(|options| options.include_icon_view_settings);
		let icon_requested = view == FinderViewStyle::Icon
			|| options.is_some_and(|options| {
				options.icon_view.is_some()
					|| options.background.is_some()
					|| options.show_item_info.is_some()
			});
		if icon_requested && icon_switch == Some(AutoFlag::Disabled) {
			return Err(Error::IconViewExcluded);
		}
		let list_switch = options.and_then(|options| options.include_list_view_settings);
		let list_requested = view == FinderViewStyle::List
			|| options.is_some_and(|options| options.list_view.is_some());
		if list_requested && list_switch == Some(AutoFlag::Disabled) {
			return Err(Error::ListViewExcluded);
		}
		let icons = (icon_requested || icon_switch == Some(AutoFlag::Enabled))
			.then(|| resolve_icons(options, image))
			.transpose()?;
		let list = (list_requested || list_switch == Some(AutoFlag::Enabled))
			.then(|| list::resolve(options))
			.transpose()?;
		Ok(Self {
			window: browser_window(options),
			view,
			icons,
			list,
			positions: options
				.and_then(|settings| settings.icon_view.as_ref())
				.map(|view| &view.positions),
		})
	}
}

fn resolve_icons(
	options: Option<&FinderOptions>,
	image: ImageBackground,
) -> Result<IconViewProperties, Error> {
	let (background_type, background_color) = resolve_background(
		options.and_then(|settings| settings.background.as_ref()),
		&image,
	)?;
	let icon_view = options.and_then(|settings| settings.icon_view.as_ref());
	let arrange_by = resolve_arrangement(icon_view.and_then(|view| view.arrange_by))?;
	let (scroll_x, scroll_y) = icon_view
		.and_then(|view| view.scroll_position)
		.map_or(DEFAULT_SCROLL_POSITION, point);
	let mut unknown = BTreeMap::from([
		(SCROLL_POSITION_X.to_owned(), PropertyList::Real(scroll_x)),
		(SCROLL_POSITION_Y.to_owned(), PropertyList::Real(scroll_y)),
	]);
	if let ImageBackground::Alias(bytes) = image {
		unknown.insert(BACKGROUND_IMAGE_ALIAS.to_owned(), PropertyList::Data(bytes));
	}
	Ok(IconViewProperties {
		arrange_by: Some(arrange_by.to_owned()),
		background_type: Some(background_type),
		background_color: Some(background_color),
		grid_offset: Some(
			icon_view
				.and_then(|view| view.grid_offset)
				.map_or(DEFAULT_GRID_OFFSET, point),
		),
		grid_spacing: Some(
			icon_view
				.and_then(|view| view.grid_spacing)
				.map_or(DEFAULT_GRID_SPACING, |size| size.get()),
		),
		icon_size: Some(
			icon_view
				.and_then(|view| view.icon_size)
				.map_or(DEFAULT_ICON_SIZE, |size| size.get()),
		),
		text_size: Some(
			icon_view
				.and_then(|view| view.text_size)
				.map_or(DEFAULT_TEXT_SIZE, |size| size.get()),
		),
		label_on_bottom: Some(
			icon_view.and_then(|view| view.label_position) != Some(LabelPosition::Right),
		),
		show_icon_preview: Some(
			options
				.and_then(|settings| settings.show_icon_preview)
				.unwrap_or_default(),
		),
		show_item_info: Some(
			options
				.and_then(|settings| settings.show_item_info)
				.unwrap_or_default(),
		),
		view_options_version: Some(VIEW_OPTIONS_VERSION),
		unknown,
		..IconViewProperties::default()
	})
}

fn browser_window(options: Option<&FinderOptions>) -> BrowserWindowSettings {
	// The project uses dmgbuild's screen-space lower-left origin for window
	// placement. Item positions have a separate folder-local, downwards axis.
	let (x, y) = options
		.and_then(|settings| settings.window_position)
		.map_or(DEFAULT_WINDOW_POSITION, point);
	let (width, height) = options
		.and_then(|settings| settings.window_size)
		.map_or(DEFAULT_WINDOW_SIZE, |[width, height]| {
			(width.get(), height.get())
		});
	BrowserWindowSettings {
		window_bounds: Some(WindowBounds {
			x,
			y,
			width,
			height,
		}),
		show_sidebar: Some(
			options
				.and_then(|settings| settings.show_sidebar)
				.unwrap_or_default(),
		),
		show_toolbar: Some(
			options
				.and_then(|settings| settings.show_toolbar)
				.unwrap_or_default(),
		),
		show_status_bar: Some(
			options
				.and_then(|settings| settings.show_status_bar)
				.unwrap_or_default(),
		),
		show_path_bar: Some(
			options
				.and_then(|settings| settings.show_pathbar)
				.unwrap_or_default(),
		),
		show_tab_view: Some(
			options
				.and_then(|settings| settings.show_tab_view)
				.unwrap_or_default(),
		),
		container_show_sidebar: Some(false),
		sidebar_width: Some(
			options
				.and_then(|settings| settings.sidebar_width)
				.map_or(DEFAULT_SIDEBAR_WIDTH, |size| size.get()),
		),
		unknown: BTreeMap::from([(
			PREVIEW_PANE_VISIBILITY.to_owned(),
			PropertyList::Boolean(false),
		)]),
		..BrowserWindowSettings::default()
	}
}

fn point([x, y]: [Coordinate; 2]) -> (f64, f64) {
	(x.get(), y.get())
}

fn resolve_arrangement(arrangement: Option<Arrangement>) -> Result<&'static str, Error> {
	let value = match arrangement {
		None | Some(Arrangement::None) => MANUAL_ARRANGEMENT,
		Some(Arrangement::Column(Column::Name)) => ARRANGE_BY_NAME,
		Some(Arrangement::Column(Column::DateModified)) => ARRANGE_BY_DATE_MODIFIED,
		Some(Arrangement::Column(Column::DateCreated)) => ARRANGE_BY_DATE_CREATED,
		Some(Arrangement::Column(Column::DateAdded)) => ARRANGE_BY_DATE_ADDED,
		Some(Arrangement::Column(Column::DateLastOpened)) => ARRANGE_BY_DATE_LAST_OPENED,
		Some(Arrangement::Column(Column::Size)) => ARRANGE_BY_SIZE,
		Some(Arrangement::Column(Column::Kind)) => ARRANGE_BY_KIND,
		Some(Arrangement::Column(Column::Label)) => ARRANGE_BY_LABEL,
		Some(arrangement) => return Err(Error::Arrangement(arrangement)),
	};
	Ok(value)
}

fn resolve_background(
	background: Option<&Background>,
	image: &ImageBackground,
) -> Result<(i64, (f64, f64, f64)), Error> {
	match background {
		None | Some(Background::None {}) => Ok((DEFAULT_BACKGROUND_TYPE, DEFAULT_BACKGROUND_RGB)),
		Some(Background::Color { value }) => Ok((
			SOLID_BACKGROUND_TYPE,
			parse_solid_background_color(value.as_str()).map_err(|_| Error::BackgroundColor)?,
		)),
		Some(Background::Image { .. }) => resolve_image_background(image),
		Some(Background::Template { name }) => {
			name.as_str()
				.parse::<BackgroundTemplate>()
				.map_err(|_| Error::TemplateBackground)?;
			resolve_image_background(image)
		}
		Some(_) => Err(Error::Background),
	}
}

fn resolve_image_background(image: &ImageBackground) -> Result<(i64, (f64, f64, f64)), Error> {
	match image {
		ImageBackground::Unavailable => Err(Error::ImageBackground),
		ImageBackground::Preflight | ImageBackground::Alias(_) => {
			Ok((IMAGE_BACKGROUND_TYPE, DEFAULT_BACKGROUND_RGB))
		}
	}
}
