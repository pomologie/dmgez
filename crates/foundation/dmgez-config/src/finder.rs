use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, de};
use thiserror::Error;

use crate::{AutoFlag, Coordinate, Dimension, LocalPath, Text, VirtualPath};

mod color;
mod template;

pub use color::{BackgroundColorError, parse_solid_background_color};
pub use template::{BackgroundTemplate, BackgroundTemplateError};

const MINIMUM_SIZE: u8 = 1;
const MINIMUM_WIDTH: u8 = 0;
type Point = [Coordinate; 2];
type Extent = Dimension<MINIMUM_SIZE>;
type Width = Dimension<MINIMUM_WIDTH>;

/// Root Finder layout, explicit folder overrides, and display selections.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Finder {
	pub options: FinderOptions,
	pub folders: Vec<Folder>,
	/// Exact destination paths whose entries Finder should mark invisible.
	pub hide: Vec<VirtualPath>,
	/// Exact destination paths to display with their extensions hidden.
	pub hide_extensions: Vec<VirtualPath>,
}

/// Finder presentation settings used for either the image root or a folder.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FinderOptions {
	pub background: Option<Background>,
	pub window_position: Option<Point>,
	pub window_size: Option<[Extent; 2]>,
	pub default_view: Option<View>,
	pub sidebar_width: Option<Width>,
	pub include_icon_view_settings: Option<AutoFlag>,
	pub include_list_view_settings: Option<AutoFlag>,
	pub icon_view: Option<IconView>,
	pub list_view: Option<ListView>,
	pub show_status_bar: Option<bool>,
	pub show_tab_view: Option<bool>,
	pub show_toolbar: Option<bool>,
	pub show_pathbar: Option<bool>,
	pub show_sidebar: Option<bool>,
	pub show_icon_preview: Option<bool>,
	pub show_item_info: Option<bool>,
}

/// Finder settings for a named folder.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Folder {
	pub path: VirtualPath,
	pub options: FinderOptions,
}

// Keep each external shape closed while sharing the common TOML keys. Serde's
// flatten attribute cannot be combined with deny_unknown_fields.
macro_rules! finder_fields {
	($name:ident, $($(#[$attribute:meta])* $field:ident: $field_type:ty),+ $(,)?) => {
		#[derive(Deserialize)]
		#[serde(deny_unknown_fields)]
		struct $name {
			background: Option<Background>,
			window_position: Option<Point>,
			window_size: Option<[Extent; 2]>,
			default_view: Option<View>,
			sidebar_width: Option<Width>,
			include_icon_view_settings: Option<AutoFlag>,
			include_list_view_settings: Option<AutoFlag>,
			icon_view: Option<IconView>,
			list_view: Option<ListView>,
			show_status_bar: Option<bool>,
			show_tab_view: Option<bool>,
			show_toolbar: Option<bool>,
			show_pathbar: Option<bool>,
			show_sidebar: Option<bool>,
			show_icon_preview: Option<bool>,
			show_item_info: Option<bool>,
			$(
				$(#[$attribute])*
				$field: $field_type,
			)+
		}

		impl $name {
			fn into_parts(self) -> (FinderOptions, $($field_type),+) {
				let options = FinderOptions {
					background: self.background,
					window_position: self.window_position,
					window_size: self.window_size,
					default_view: self.default_view,
					sidebar_width: self.sidebar_width,
					include_icon_view_settings: self.include_icon_view_settings,
					include_list_view_settings: self.include_list_view_settings,
					icon_view: self.icon_view,
					list_view: self.list_view,
					show_status_bar: self.show_status_bar,
					show_tab_view: self.show_tab_view,
					show_toolbar: self.show_toolbar,
					show_pathbar: self.show_pathbar,
					show_sidebar: self.show_sidebar,
					show_icon_preview: self.show_icon_preview,
					show_item_info: self.show_item_info,
				};
				(options, $(self.$field),+)
			}
		}
	};
}

finder_fields!(
	RawFinder,
	#[serde(default)] folders: Vec<Folder>,
	#[serde(default)] hide: Vec<VirtualPath>,
	#[serde(default)] hide_extensions: Vec<VirtualPath>,
);
finder_fields!(RawFolder, path: VirtualPath);

impl<'de> Deserialize<'de> for Finder {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let (options, folders, hide, hide_extensions) =
			RawFinder::deserialize(deserializer)?.into_parts();
		if !distinct_children(folders.iter().map(|folder| &folder.path)) {
			return Err(de::Error::custom(
				"Finder folder paths must be distinct non-root paths",
			));
		}
		if !distinct_children(&hide) || !distinct_children(&hide_extensions) {
			return Err(de::Error::custom(
				"each Finder visibility selection must contain distinct non-root paths",
			));
		}
		Ok(Self {
			options,
			folders,
			hide,
			hide_extensions,
		})
	}
}

fn distinct_children<'path>(paths: impl IntoIterator<Item = &'path VirtualPath>) -> bool {
	let mut seen = BTreeSet::new();
	paths
		.into_iter()
		.all(|path| !path.is_root() && seen.insert(path))
}

impl<'de> Deserialize<'de> for Folder {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let (options, path) = RawFolder::deserialize(deserializer)?.into_parts();
		Ok(Self { path, options })
	}
}

/// Recognized Finder background settings.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Background {
	Image {
		source: LocalPath,
		retina: Option<LocalPath>,
	},
	Color {
		value: Text,
	},
	Template {
		name: Text,
	},
	None {},
}

/// Recognized default Finder views.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum View {
	IconView,
	ListView,
	ColumnView,
	Coverflow,
	Gallery,
}

/// Columns and sort keys recognized by the Finder settings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum Column {
	Name,
	DateModified,
	DateCreated,
	DateAdded,
	DateLastOpened,
	Size,
	Kind,
	Label,
	Version,
	Comments,
}

/// Finder icon arrangement policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Arrangement {
	None,
	Column(Column),
}

impl<'de> Deserialize<'de> for Arrangement {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		#[serde(rename_all = "lowercase")]
		enum NoArrangement {
			None,
		}

		#[derive(Deserialize)]
		#[serde(untagged)]
		enum Input {
			None(NoArrangement),
			Column(Column),
		}

		Ok(match Input::deserialize(deserializer)? {
			Input::None(NoArrangement::None) => Self::None,
			Input::Column(column) => Self::Column(column),
		})
	}
}

/// Placement of text labels in icon view.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum LabelPosition {
	Bottom,
	Right,
}

/// Sort direction for a Finder list column.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Direction {
	Ascending,
	Descending,
}

/// Icon view presentation settings.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct IconView {
	pub arrange_by: Option<Arrangement>,
	pub grid_offset: Option<Point>,
	pub grid_spacing: Option<Extent>,
	pub scroll_position: Option<Point>,
	pub label_position: Option<LabelPosition>,
	pub text_size: Option<Extent>,
	pub icon_size: Option<Extent>,
	#[serde(default)]
	pub positions: BTreeMap<String, Point>,
}

/// List view presentation settings.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct ListView {
	pub icon_size: Option<Extent>,
	pub text_size: Option<Extent>,
	pub scroll_position: Option<Point>,
	pub sort_by: Option<Column>,
	pub use_relative_dates: Option<bool>,
	pub calculate_all_sizes: Option<bool>,
	#[serde(default, deserialize_with = "unique_columns")]
	pub columns: Option<Vec<Column>>,
	#[serde(default)]
	pub column_widths: BTreeMap<Column, Extent>,
	#[serde(default)]
	pub sort_directions: BTreeMap<Column, Direction>,
}

fn unique_columns<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<Option<Vec<Column>>, D::Error> {
	let columns = Vec::<Column>::deserialize(deserializer)?;
	if columns.is_empty() || columns.iter().collect::<BTreeSet<_>>().len() != columns.len() {
		return Err(de::Error::custom(
			"Finder columns must be nonempty and distinct",
		));
	}
	Ok(Some(columns))
}

/// An icon source declares exactly one representation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum VolumeIcon {
	Source(LocalPath),
	Images(BTreeMap<IconSlot, LocalPath>),
	Badge(LocalPath),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawVolumeIcon {
	source: Option<LocalPath>,
	images: Option<BTreeMap<IconSlot, LocalPath>>,
	badge: Option<LocalPath>,
}

impl TryFrom<RawVolumeIcon> for VolumeIcon {
	type Error = VolumeIconError;

	fn try_from(raw: RawVolumeIcon) -> Result<Self, Self::Error> {
		match (raw.source, raw.images, raw.badge) {
			(Some(source), None, None) => Ok(Self::Source(source)),
			(None, Some(images), None) if !images.is_empty() => Ok(Self::Images(images)),
			(None, None, Some(badge)) => Ok(Self::Badge(badge)),
			_ => Err(VolumeIconError::Representation),
		}
	}
}

/// Invalid volume icon settings.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum VolumeIconError {
	#[error("volume icon requires exactly one source, nonempty image set or badge")]
	Representation,
}

/// Recognized image slots for a volume icon.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum IconSlot {
	#[serde(rename = "16")]
	Small,
	#[serde(rename = "32")]
	Medium,
	#[serde(rename = "128")]
	Large,
	#[serde(rename = "256")]
	Larger,
	#[serde(rename = "512")]
	Largest,
	#[serde(rename = "16@2x")]
	SmallRetina,
	#[serde(rename = "32@2x")]
	MediumRetina,
	#[serde(rename = "128@2x")]
	LargeRetina,
	#[serde(rename = "256@2x")]
	LargerRetina,
	#[serde(rename = "512@2x")]
	LargestRetina,
}
