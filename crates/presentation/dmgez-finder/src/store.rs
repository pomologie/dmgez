use dmg_layout::{AliasKind, AliasRecord, DiskImageError};
use dmgez_config::{Arrangement, Background, Column, Coordinate, FinderOptions, View};
use thiserror::Error;

use crate::{
	background_alias::BackgroundAliasError,
	layout::{ImageBackground, Layout},
	records::Store,
};

/// The generated Finder metadata filename, reserved before catalog admission.
pub const STORE_NAME: &str = ".DS_Store";

/// A direct child of the folder whose Finder metadata is being generated.
#[derive(Clone, Copy, Debug)]
pub struct LayoutItem<'a> {
	/// The final on-volume catalog name.
	pub name: &'a str,
	/// Optional icon center in folder-local points, measured rightward and down.
	pub position: Option<[Coordinate; 2]>,
}

/// A layout cannot be preserved within the supported Finder store profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	/// The selected default view is outside the supported writing profile.
	#[error("Finder default view {0:?} is unavailable")]
	View(View),
	/// Required or explicitly requested icon-view options cannot be excluded.
	#[error("the requested Finder layout requires icon-view settings")]
	IconViewExcluded,
	/// Required or explicitly requested list-view options cannot be excluded.
	#[error("the requested Finder layout requires list-view settings")]
	ListViewExcluded,
	/// Finder has no qualified wire identifier for the configured column.
	#[error("Finder list column {0:?} is unavailable")]
	ListColumn(Column),
	/// The visible list column order must be nonempty and unique.
	#[error("Finder list columns must be nonempty and distinct")]
	ListColumnOrder,
	/// Finder stores column widths in positive whole points.
	#[error(
		"Finder list column {0:?} requires a positive whole-point width within the signed 32-bit range"
	)]
	ListColumnWidth(Column),
	/// Finder has no qualified encoding for the configured sort direction.
	#[error("Finder list column {0:?} has an unsupported sort direction")]
	ListSortDirection(Column),
	/// Image backgrounds need a generated alias targeting sealed volume identities.
	#[error("Finder image background aliases are unavailable")]
	ImageBackground,
	/// An image-background operation requires an image setting in this folder.
	#[error("Finder layout does not specify an image background")]
	ImageBackgroundNotSelected,
	/// A background image alias must resolve to a file.
	#[error("Finder background image alias targets a folder")]
	ImageBackgroundTarget,
	/// The supplied alias record could not be serialized.
	#[error("could not encode Finder background image alias")]
	ImageBackgroundAlias(#[source] DiskImageError),
	/// The generated background alias could not represent the supplied identity.
	#[error(transparent)]
	ImageBackgroundIdentity(#[from] BackgroundAliasError),
	/// Built-in template backgrounds are outside the initial profile.
	#[error("Finder template backgrounds are unavailable")]
	TemplateBackground,
	/// A newer configuration selected a background outside this profile.
	#[error("Finder background option is unavailable")]
	Background,
	/// The initial color profile accepts opaque hexadecimal RGB.
	#[error("Finder background color must use #RGB or #RRGGBB")]
	BackgroundColor,
	/// The arrangement key has no qualified icon-view spelling.
	#[error("Finder icon arrangement by {0:?} is unavailable")]
	Arrangement(Arrangement),
	/// The item name must be one printable ASCII catalog component.
	#[error("Finder item name is outside the printable ASCII catalog profile")]
	ItemName,
	/// A folder-level position must name an existing direct child exactly.
	#[error("Finder icon position refers to absent item {0:?}")]
	MissingItem(String),
	/// Item-level and folder-level inputs both declare a position for one name.
	#[error("Finder icon position for {0:?} is declared more than once")]
	DuplicatePosition(String),
	/// Two output records would refer to case-equivalent item names.
	#[error("Finder icon names collide at {0:?}")]
	DuplicateName(String),
	/// The binary icon position cannot preserve the requested coordinate exactly.
	#[error("Finder icon position for {0:?} requires nonnegative whole points")]
	Position(String),
	/// The desired records do not fit the qualified single-leaf profile.
	#[error("Finder layout exceeds the single-page store budget")]
	StoreBudget,
	/// A property list could not be serialized.
	#[error("could not encode Finder view settings")]
	PropertyList(#[from] plist::Error),
	/// The underlying store writer rejected the records.
	#[error("could not encode Finder store")]
	Store(#[from] ds_parser::Error),
}

/// Checks view and background options before accessing any image content.
///
/// # Errors
///
/// Returns an error for a view, background, arrangement, or record switch
/// outside the qualified icon and list profile. Use [`write_store`] to check
/// item positions against the resolved folder children.
pub fn validate_options(options: Option<&FinderOptions>) -> Result<(), Error> {
	Layout::resolve(options, ImageBackground::Unavailable).map(|_| ())
}

/// Checks a layout whose image background will be bound after assigning catalog IDs.
///
/// This check neither opens the configured image nor constructs an alias. Use
/// [`write_store_with_background_alias`] when the final file identity is known.
///
/// # Errors
///
/// Returns an error if options select a different background or the remaining
/// layout options fall outside the writer's supported profile.
pub fn validate_options_for_image_background(options: &FinderOptions) -> Result<(), Error> {
	validate_image_background(options)?;
	Layout::resolve(Some(options), ImageBackground::Preflight).map(|_| ())
}

/// Creates a new `.DS_Store` for the root or another image folder.
///
/// Item positions are relative to that folder. Configured folder positions
/// must match a final child name exactly. Automatic icon arrangement can
/// override the positions when Finder displays the folder.
///
/// # Errors
///
/// Returns an error for unsupported settings, unresolved or conflicting
/// positions, unrepresentable items, or a store larger than one leaf.
pub fn write_store(
	options: Option<&FinderOptions>,
	items: &[LayoutItem<'_>],
) -> Result<Vec<u8>, Error> {
	let layout = Layout::resolve(options, ImageBackground::Unavailable)?;
	write_layout(layout, items)
}

/// Creates a Finder store with an image background bound to a final volume file.
///
/// The alias must describe the on-volume image file, with its final volume and
/// catalog identities. The caller supplies that alias after reserving the
/// image file and store. The writer never resolves the configured source or
/// the alias to a host path.
///
/// # Errors
///
/// Returns an error for a missing image setting, a folder target, an invalid
/// alias, or any layout, item-position, or store limit described by [`write_store`].
pub fn write_store_with_background_alias(
	options: &FinderOptions,
	items: &[LayoutItem<'_>],
	alias: &AliasRecord,
) -> Result<Vec<u8>, Error> {
	validate_image_background(options)?;
	if alias.header.kind != AliasKind::File {
		return Err(Error::ImageBackgroundTarget);
	}
	let bytes = alias.to_bytes().map_err(Error::ImageBackgroundAlias)?;
	let layout = Layout::resolve(Some(options), ImageBackground::Alias(bytes))?;
	write_layout(layout, items)
}

fn validate_image_background(options: &FinderOptions) -> Result<(), Error> {
	if matches!(options.background, Some(Background::Image { .. })) {
		Ok(())
	} else {
		Err(Error::ImageBackgroundNotSelected)
	}
}

fn write_layout(layout: Layout<'_>, items: &[LayoutItem<'_>]) -> Result<Vec<u8>, Error> {
	let positions = layout.positions;
	let mut store = Store::for_layout(layout)?;
	store.position_items(items, positions)?;
	store.finish()
}
