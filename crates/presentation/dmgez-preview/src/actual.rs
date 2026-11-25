//! Actual-image preview inputs and coverage based on one observed directory.

mod coverage;
mod style;

use std::{collections::BTreeMap, collections::TryReserveError};

use dmgez_finder::inspection::{
	BackgroundAlias, IconView, ReadLimits, RecordValue, Report, ViewKind, Window,
};
use thiserror::Error;

use crate::{
	FolderPoint, FolderRect, FolderStyle, GeometryError, ItemVisibility,
	project::{display_name, fallback_center},
};

pub use coverage::{BackgroundStatus, Coverage, LayoutStatus, StoreStatus};

const SELF_RECORD: &str = ".";
const MANUAL_ARRANGEMENT: &str = "none";
const DEFAULT_BACKGROUND_TYPE: i64 = 0;
const COLOR_BACKGROUND_TYPE: i64 = 1;
const IMAGE_BACKGROUND_TYPE: i64 = 2;
const MAX_ENTRIES: usize = 512;
const MAX_NAME_UNITS: usize = 255;
// Each observed icon can cover at most three grid slots per axis when the
// grid step is at least one icon extent.
const MAX_FALLBACK_SLOTS: usize = MAX_ENTRIES * (1 + 3 * 3);

/// One observed direct child of the selected directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
	/// Exact stored name. No case folding or normalization is applied.
	pub name: Vec<u16>,
	/// Entry kind from the catalog observation.
	pub kind: EntryKind,
	/// Whether the source view considers the entry hidden.
	pub hidden: bool,
	/// Whether the stored metadata requests hiding the label's extension.
	pub hide_extension: bool,
}

/// The observed kind of a directory entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
	Directory,
	File,
	Symlink,
	Other,
}

/// The caller's admission result for the selected directory's Finder store.
#[derive(Clone, Copy, Debug)]
pub enum Store<'a> {
	Absent,
	Unavailable,
	Unsupported,
	Malformed,
	Observed(&'a Report),
}

impl Store<'_> {
	const fn status(self) -> StoreStatus {
		match self {
			Self::Absent => StoreStatus::Absent,
			Self::Unavailable => StoreStatus::Unavailable,
			Self::Unsupported => StoreStatus::Unsupported,
			Self::Malformed => StoreStatus::Malformed,
			Self::Observed(_) => StoreStatus::Observed,
		}
	}
}

/// In-image bytes obtained by checking a background alias against its catalog identity.
#[derive(Clone, Copy, Debug)]
pub enum BackgroundAsset<'a> {
	Unavailable,
	/// Bytes from the same immutable directory and volume observation as the store.
	Encoded(&'a [u8]),
}

/// The observed directory does not fit the preview profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("observed folder exceeds the preview limit of {MAX_ENTRIES} entries")]
	EntryBudget,
	#[error("observed entry name exceeds the supported name size or is empty")]
	Name,
	#[error("observed folder contains a duplicate stored name")]
	DuplicateEntry,
	#[error("fallback icon placement exceeded its work budget")]
	PlacementBudget,
	#[error(transparent)]
	Geometry(#[from] GeometryError),
	#[error("could not allocate the observed icon layout")]
	Allocation(#[from] TryReserveError),
}

pub(crate) struct Scene<'a> {
	pub(crate) style: FolderStyle<'static>,
	pub(crate) items: Vec<Item<'a>>,
	pub(crate) background: Background<'a>,
	pub(crate) coverage: Coverage,
}

pub(crate) struct Item<'a> {
	pub(crate) entry: &'a Entry,
	display: String,
	pub(crate) bounds: FolderRect,
}

impl Item<'_> {
	pub(crate) fn label(&self) -> &str {
		display_name(&self.display, self.entry.hide_extension)
	}

	pub(crate) fn is_directory(&self) -> bool {
		self.entry.kind == EntryKind::Directory
	}
}

pub(crate) enum Background<'a> {
	Default(BackgroundStatus),
	Color([f64; 3]),
	Image(&'a [u8]),
}

impl<'a> Scene<'a> {
	pub(crate) fn from_image(
		entries: &'a [Entry],
		store: Store<'_>,
		asset: BackgroundAsset<'a>,
		visibility: ItemVisibility,
	) -> Result<Self, Error> {
		let entries = ordered_entries(entries)?;
		let (style, layout, positions, background) = settings(store, asset)?;
		let mut coverage = Coverage {
			store: store.status(),
			layout,
			background: BackgroundStatus::Default,
			observed_positions: 0,
			fallback_positions: 0,
		};
		let mut items = Vec::new();
		items.try_reserve_exact(entries.len())?;
		let mut occupied = Vec::new();
		occupied.try_reserve_exact(entries.len())?;
		for entry in &entries {
			if let Some(bounds) = stored_bounds(&positions, entry, style)? {
				occupied.push(bounds);
			}
		}
		let mut slots = (0..MAX_FALLBACK_SLOTS)
			.map(|index| FolderRect::centered(fallback_center(style, index)?, style.icon_size));
		for entry in entries {
			let visible = !entry.hidden || visibility == ItemVisibility::IncludeHidden;
			let bounds = if let Some(bounds) = stored_bounds(&positions, entry, style)? {
				coverage.observed_positions += usize::from(visible);
				bounds
			} else {
				coverage.fallback_positions += usize::from(visible);
				loop {
					let candidate = slots.next().ok_or(Error::PlacementBudget)??;
					if occupied.iter().all(|seen| !overlaps(*seen, candidate)) {
						break candidate;
					}
				}
			};
			if visible {
				items.push(Item {
					entry,
					display: String::from_utf16_lossy(&entry.name),
					bounds,
				});
			}
		}
		if coverage.layout == LayoutStatus::ObservedIcon && coverage.fallback_positions != 0 {
			coverage.layout = LayoutStatus::PartialIcon;
		}
		Ok(Self {
			style,
			items,
			background,
			coverage,
		})
	}
}

fn stored_bounds(
	positions: &Positions,
	entry: &Entry,
	style: FolderStyle<'_>,
) -> Result<Option<FolderRect>, GeometryError> {
	positions
		.get(entry.name.as_slice())
		.map(|&[x, y]| {
			let center = FolderPoint::new(f64::from(x), f64::from(y))?;
			FolderRect::centered(center, style.icon_size)
		})
		.transpose()
}

fn overlaps(left: FolderRect, right: FolderRect) -> bool {
	left.min().x() < right.max().x()
		&& right.min().x() < left.max().x()
		&& left.min().y() < right.max().y()
		&& right.min().y() < left.max().y()
}

fn ordered_entries(entries: &[Entry]) -> Result<Vec<&Entry>, Error> {
	if entries.len() > MAX_ENTRIES {
		return Err(Error::EntryBudget);
	}
	if entries
		.iter()
		.any(|entry| entry.name.is_empty() || entry.name.len() > MAX_NAME_UNITS)
	{
		return Err(Error::Name);
	}
	let mut ordered = Vec::new();
	ordered.try_reserve_exact(entries.len())?;
	ordered.extend(entries);
	ordered.sort_unstable_by(|a, b| a.name.cmp(&b.name));
	if ordered.windows(2).any(|pair| pair[0].name == pair[1].name) {
		return Err(Error::DuplicateEntry);
	}
	Ok(ordered)
}

type Positions = BTreeMap<Vec<u16>, [u32; 2]>;
type Settings<'a> = (
	FolderStyle<'static>,
	LayoutStatus,
	Positions,
	Background<'a>,
);

fn settings<'a>(
	store: Store<'_>,
	asset: BackgroundAsset<'a>,
) -> Result<Settings<'a>, GeometryError> {
	let initial = style::fallback()?;
	let fallback = |status| {
		(
			initial,
			status,
			Positions::new(),
			Background::Default(BackgroundStatus::Default),
		)
	};
	let Store::Observed(report) = store else {
		return Ok(fallback(LayoutStatus::GenericNoStore));
	};
	let Some(observed) = StoreRecords::read(report) else {
		return Ok(fallback(LayoutStatus::GenericInvalidSettings));
	};
	if matches!(observed.view, Some(ViewKind::Other(_))) {
		return Ok(fallback(LayoutStatus::GenericOtherView));
	}
	if observed
		.icons
		.and_then(|icon| icon.arrangement.as_deref())
		.is_some_and(|kind| kind != MANUAL_ARRANGEMENT)
	{
		return Ok(fallback(LayoutStatus::GenericAutomaticArrangement));
	}
	let Ok((style, complete)) = style::observed(initial, observed.window, observed.icons) else {
		return Ok(fallback(LayoutStatus::GenericInvalidSettings));
	};
	let layout = if complete && observed.view == Some(ViewKind::Icon) {
		LayoutStatus::ObservedIcon
	} else {
		LayoutStatus::PartialIcon
	};
	Ok((
		style,
		layout,
		observed.positions,
		background(observed.icons, asset),
	))
}

#[derive(Default)]
struct StoreRecords<'a> {
	view: Option<ViewKind>,
	window: Option<&'a Window>,
	icons: Option<&'a IconView>,
	positions: Positions,
}

impl<'a> StoreRecords<'a> {
	fn read(report: &'a Report) -> Option<Self> {
		let limits = ReadLimits::default();
		if report.records.len() > limits.records {
			return None;
		}
		let mut result = Self::default();
		for record in &report.records {
			let duplicate = match &record.value {
				RecordValue::View(view) if record.name == SELF_RECORD => {
					result.view.replace(*view).is_some()
				}
				RecordValue::Window(window) if record.name == SELF_RECORD => {
					result.window.replace(window).is_some()
				}
				RecordValue::IconView(icons) if record.name == SELF_RECORD => {
					result.icons.replace(icons).is_some()
				}
				RecordValue::IconPosition { x, y } if record.name != SELF_RECORD => {
					let name: Vec<_> = record
						.name
						.encode_utf16()
						.take(MAX_NAME_UNITS + 1)
						.collect();
					if name.len() > MAX_NAME_UNITS || name.is_empty() {
						return None;
					}
					result.positions.insert(name, [*x, *y]).is_some()
				}
				_ => false,
			};
			if duplicate {
				return None;
			}
		}
		Some(result)
	}
}

fn background<'a>(icons: Option<&IconView>, asset: BackgroundAsset<'a>) -> Background<'a> {
	let Some(icons) = icons else {
		return Background::Default(BackgroundStatus::Default);
	};
	match icons.background_type {
		None | Some(DEFAULT_BACKGROUND_TYPE) => Background::Default(BackgroundStatus::Default),
		Some(COLOR_BACKGROUND_TYPE) => match icons.background_color {
			Some(color) if color.iter().all(|channel| (0.0..=1.0).contains(channel)) => {
				Background::Color(color)
			}
			_ => Background::Default(BackgroundStatus::InvalidSetting),
		},
		Some(IMAGE_BACKGROUND_TYPE) => match (&icons.background_alias, asset) {
			(Some(BackgroundAlias::File(_)), BackgroundAsset::Encoded(bytes)) => {
				Background::Image(bytes)
			}
			(Some(BackgroundAlias::Malformed), _) => {
				Background::Default(BackgroundStatus::InvalidSetting)
			}
			(
				Some(BackgroundAlias::UnsupportedVersion(_) | BackgroundAlias::UnsupportedTarget),
				_,
			) => Background::Default(BackgroundStatus::UnsupportedSetting),
			_ => Background::Default(BackgroundStatus::UnavailableImage),
		},
		Some(_) => Background::Default(BackgroundStatus::UnsupportedSetting),
	}
}
