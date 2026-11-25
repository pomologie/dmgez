use std::collections::BTreeMap;

use dmgez_config::{
	Arrangement, Background, Content, Coordinate, FinderOptions, LabelPosition, Project, View,
	VirtualPath,
};
use thiserror::Error;

use crate::{
	CanvasPoint, FinderScreenPoint, FolderPoint, FolderRect, GeometryError, Grid, Size, Viewport,
};

const IMAGE_ROOT: &str = "/";
const PATH_SEPARATOR: char = '/';
// These design defaults match the Finder writer and the Finder feature contract.
const DEFAULT_WINDOW_POSITION: (f64, f64) = (100.0, 100.0);
const DEFAULT_WINDOW_SIZE: (f64, f64) = (640.0, 280.0);
const DEFAULT_GRID_SPACING: f64 = 80.0;
const DEFAULT_ICON_SIZE: f64 = 128.0;
const DEFAULT_TEXT_SIZE: f64 = 16.0;

/// Project metadata could not be projected into a folder layout.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LayoutError {
	#[error("the selected folder has more than one Finder override")]
	AmbiguousFolder,
	#[error("the selected folder uses unsupported view {0:?}")]
	UnsupportedView(View),
	#[error("preview item is not a direct child of the selected folder: {0:?}")]
	NotChild(String),
	#[error("preview item is duplicated: {0:?}")]
	DuplicateItem(String),
	#[error("folder position refers to a missing direct child: {0:?}")]
	MissingItem(String),
	#[error("position is configured in both the content and Finder tables: {0:?}")]
	DuplicatePosition(String),
	#[error(transparent)]
	Geometry(#[from] GeometryError),
}

/// One resolved direct child and its design-side content overrides.
/// The caller supplies destination names from the logical image tree.
#[derive(Clone, Copy, Debug)]
pub struct ItemInput<'project> {
	pub destination: &'project VirtualPath,
	pub position: Option<[Coordinate; 2]>,
	pub hidden: bool,
	pub hide_extension: bool,
}

impl<'project> From<&'project Content> for ItemInput<'project> {
	fn from(content: &'project Content) -> Self {
		Self {
			destination: &content.destination,
			position: content.position,
			hidden: content.hidden.unwrap_or_default(),
			hide_extension: content.hide_extension.unwrap_or_default(),
		}
	}
}

/// Which policy supplied the final preview position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionSource {
	Content,
	FinderFolder,
	/// A deterministic design placement for an entry without a configured position.
	PreviewGrid,
}

/// Icon and label inputs shared by the interactive and headless consumers.
#[derive(Clone, Copy, Debug)]
pub struct PlacedItem<'project> {
	pub destination: &'project VirtualPath,
	pub name: &'project str,
	pub center: FolderPoint,
	pub icon_bounds: FolderRect,
	pub position_source: PositionSource,
	pub hidden: bool,
	pub hide_extension: bool,
}

impl<'project> PlacedItem<'project> {
	/// Returns the display name after applying the configured extension visibility.
	#[must_use]
	pub fn display_name(self) -> &'project str {
		display_name(self.name, self.hide_extension)
	}
}

pub(crate) fn display_name(name: &str, hide_extension: bool) -> &str {
	const EXTENSION_SEPARATOR: char = '.';
	if hide_extension
		&& let Some((stem, _)) = name.rsplit_once(EXTENSION_SEPARATOR)
		&& !stem.is_empty()
	{
		return stem;
	}
	name
}

/// Selection policy for items marked hidden in the image metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemVisibility {
	Visible,
	IncludeHidden,
}

/// Logical style resolved from root defaults and folder-specific overrides.
#[derive(Clone, Copy, Debug)]
pub struct FolderStyle<'project> {
	pub window_origin: FinderScreenPoint,
	pub window_size: Size,
	pub scroll: FolderPoint,
	pub grid_offset: FolderPoint,
	pub grid_spacing: f64,
	pub icon_size: Size,
	pub text_size: f64,
	pub label_position: LabelPosition,
	pub arrangement: Arrangement,
	pub background: Option<&'project Background>,
}

impl FolderStyle<'_> {
	/// Creates the initial viewport for this folder using its configured scroll.
	/// # Errors
	/// Rejects an invalid canvas scale.
	pub fn viewport(self, origin: CanvasPoint, scale: f64) -> Result<Viewport, GeometryError> {
		Viewport::new(origin, self.scroll, scale)
	}

	/// Returns the design grid used to snap icon centers.
	/// # Errors
	/// Rejects a center grid origin outside the representable domain.
	pub fn snap_grid(self) -> Result<Grid, GeometryError> {
		Grid::new(self.grid_origin()?, self.grid_spacing)
	}

	fn grid_origin(self) -> Result<FolderPoint, GeometryError> {
		FolderPoint::new(
			self.grid_offset.x() + self.icon_size.width() / 2.0,
			self.grid_offset.y() + self.icon_size.height() / 2.0,
		)
	}
}

/// The design geometry of one selected folder. This takes previously
/// resolved virtual entries and performs no file access.
#[derive(Debug)]
pub struct ProjectFolder<'project> {
	style: FolderStyle<'project>,
	items: Vec<PlacedItem<'project>>,
}

impl<'project> ProjectFolder<'project> {
	/// Resolves one icon-view folder from the typed project and its direct
	/// children. Root style applies to nested folders with per-property
	/// overrides. Position tables stay local to their own folder.
	///
	/// Unpositioned items use a design-only grid in lexical name order.
	/// Positioned items keep their exact centers even when they overlap,
	/// extend outside the viewport, or use fractional coordinates.
	/// # Errors
	/// Rejects unsupported views, conflicting folder settings, invalid child
	/// data and unrepresentable geometry.
	pub fn from_project(
		project: &'project Project,
		folder: &VirtualPath,
		children: &[ItemInput<'project>],
	) -> Result<Self, LayoutError> {
		let root = project.finder.as_ref().map(|finder| &finder.options);
		let local = folder_options(project, folder, root)?;
		let style = resolve_style(root, local)?;
		let configured = local
			.and_then(|options| options.icon_view.as_ref())
			.map(|view| &view.positions);

		let mut named_children = BTreeMap::new();
		for child in children {
			let path = child.destination.as_str();
			let (parent, name) = path
				.rsplit_once(PATH_SEPARATOR)
				.filter(|(_, name)| !name.is_empty())
				.ok_or_else(|| LayoutError::NotChild(path.to_owned()))?;
			let parent = if parent.is_empty() {
				IMAGE_ROOT
			} else {
				parent
			};
			if parent != folder.as_str() {
				return Err(LayoutError::NotChild(path.to_owned()));
			}
			if named_children.insert(name, child).is_some() {
				return Err(LayoutError::DuplicateItem(path.to_owned()));
			}
		}
		if let Some(positions) = configured {
			for name in positions.keys() {
				if !named_children.contains_key(name.as_str()) {
					return Err(LayoutError::MissingItem(name.clone()));
				}
			}
		}

		let mut items = Vec::with_capacity(named_children.len());
		let mut fallback_index = 0;
		for (name, child) in named_children {
			let local_position = configured
				.and_then(|positions| positions.get(name))
				.copied();
			let (center, position_source) = match (child.position, local_position) {
				(Some(_), Some(_)) => return Err(LayoutError::DuplicatePosition(name.to_owned())),
				(Some(position), None) => (point(position)?, PositionSource::Content),
				(None, Some(position)) => (point(position)?, PositionSource::FinderFolder),
				(None, None) => {
					let center = fallback_center(style, fallback_index)?;
					fallback_index += 1;
					(center, PositionSource::PreviewGrid)
				}
			};
			items.push(PlacedItem {
				destination: child.destination,
				name,
				center,
				icon_bounds: FolderRect::centered(center, style.icon_size)?,
				position_source,
				hidden: child.hidden,
				hide_extension: child.hide_extension,
			});
		}
		Ok(Self { style, items })
	}

	#[must_use]
	pub const fn style(&self) -> FolderStyle<'project> {
		self.style
	}

	/// Returns icons in lexical painting order.
	#[must_use]
	pub fn items(&self) -> &[PlacedItem<'project>] {
		&self.items
	}

	/// Returns the viewport rectangle in folder-local units.
	/// # Errors
	/// Returns an error for an unrepresentable viewport rectangle.
	pub fn visible_bounds(&self, viewport: Viewport) -> Result<FolderRect, GeometryError> {
		FolderRect::from_top_left(viewport.folder_origin(), self.style.window_size)
	}

	/// Hits the topmost icon inside the visible content window.
	/// # Errors
	/// Returns an error for unrepresentable coordinates or viewport bounds.
	pub fn hit_test(
		&self,
		pointer: CanvasPoint,
		viewport: Viewport,
		visibility: ItemVisibility,
	) -> Result<Option<&PlacedItem<'project>>, GeometryError> {
		let pointer = viewport.to_folder(pointer)?;
		if !self.visible_bounds(viewport)?.contains(pointer) {
			return Ok(None);
		}
		Ok(self.items.iter().rev().find(|item| {
			(!item.hidden || visibility == ItemVisibility::IncludeHidden)
				&& item.icon_bounds.contains(pointer)
		}))
	}
}

fn folder_options<'project>(
	project: &'project Project,
	folder: &VirtualPath,
	root: Option<&'project FinderOptions>,
) -> Result<Option<&'project FinderOptions>, LayoutError> {
	if folder.is_root() {
		return Ok(root);
	}
	let Some(finder) = &project.finder else {
		return Ok(None);
	};
	let mut matches = finder
		.folders
		.iter()
		.filter(|item| item.path.as_str().eq_ignore_ascii_case(folder.as_str()));
	let local = matches.next().map(|item| &item.options);
	if matches.next().is_some() {
		return Err(LayoutError::AmbiguousFolder);
	}
	Ok(local)
}

fn resolve_style<'project>(
	root: Option<&'project FinderOptions>,
	local: Option<&'project FinderOptions>,
) -> Result<FolderStyle<'project>, LayoutError> {
	let (local_icons, root_icons) = (
		local.and_then(|options| options.icon_view.as_ref()),
		root.and_then(|options| options.icon_view.as_ref()),
	);
	let view = local
		.and_then(|options| options.default_view)
		.or_else(|| root.and_then(|options| options.default_view))
		.unwrap_or(View::IconView);
	if view != View::IconView {
		return Err(LayoutError::UnsupportedView(view));
	}
	let (x, y) = local
		.and_then(|options| options.window_position)
		.or_else(|| root.and_then(|options| options.window_position))
		.map_or(DEFAULT_WINDOW_POSITION, |[x, y]| (x.get(), y.get()));
	let (width, height) = local
		.and_then(|options| options.window_size)
		.or_else(|| root.and_then(|options| options.window_size))
		.map_or(DEFAULT_WINDOW_SIZE, |[width, height]| {
			(width.get(), height.get())
		});
	let (scroll_x, scroll_y) = local_icons
		.and_then(|view| view.scroll_position)
		.or_else(|| root_icons.and_then(|view| view.scroll_position))
		.map_or((0.0, 0.0), |[x, y]| (x.get(), y.get()));
	let (offset_x, offset_y) = local_icons
		.and_then(|view| view.grid_offset)
		.or_else(|| root_icons.and_then(|view| view.grid_offset))
		.map_or((0.0, 0.0), |[x, y]| (x.get(), y.get()));
	let grid_spacing = local_icons
		.and_then(|view| view.grid_spacing)
		.or_else(|| root_icons.and_then(|view| view.grid_spacing))
		.map_or(DEFAULT_GRID_SPACING, |size| size.get());
	let icon_size = local_icons
		.and_then(|view| view.icon_size)
		.or_else(|| root_icons.and_then(|view| view.icon_size))
		.map_or(DEFAULT_ICON_SIZE, |size| size.get());
	let text_size = local_icons
		.and_then(|view| view.text_size)
		.or_else(|| root_icons.and_then(|view| view.text_size))
		.map_or(DEFAULT_TEXT_SIZE, |size| size.get());
	let label_position = local_icons
		.and_then(|view| view.label_position)
		.or_else(|| root_icons.and_then(|view| view.label_position))
		.unwrap_or(LabelPosition::Bottom);
	let arrangement = local_icons
		.and_then(|view| view.arrange_by)
		.or_else(|| root_icons.and_then(|view| view.arrange_by))
		.unwrap_or(Arrangement::None);
	let background = local
		.and_then(|options| options.background.as_ref())
		.or_else(|| root.and_then(|options| options.background.as_ref()));
	Ok(FolderStyle {
		window_origin: FinderScreenPoint::new(x, y)?,
		window_size: Size::new(width, height)?,
		scroll: FolderPoint::new(scroll_x, scroll_y)?,
		grid_offset: FolderPoint::new(offset_x, offset_y)?,
		grid_spacing,
		icon_size: Size::new(icon_size, icon_size)?,
		text_size,
		label_position,
		arrangement,
		background,
	})
}

fn point([x, y]: [Coordinate; 2]) -> Result<FolderPoint, GeometryError> {
	FolderPoint::new(x.get(), y.get())
}

pub(crate) fn fallback_center(
	style: FolderStyle<'_>,
	index: usize,
) -> Result<FolderPoint, GeometryError> {
	let step = style.grid_spacing.max(style.icon_size.width());
	let columns = ((style.window_size.width() - style.icon_size.width()).max(0.0) / step).floor();
	let columns = (columns as usize).saturating_add(1);
	let origin = style.grid_origin()?;
	FolderPoint::new(
		step.mul_add((index % columns) as f64, origin.x()),
		step.mul_add((index / columns) as f64, origin.y()),
	)
}
