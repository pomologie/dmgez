use std::collections::BTreeSet;

use dmgez_config::{Project, VirtualPath, project_edit::ProjectEdit};
use dmgez_preview::{FolderPoint, PlacedItem, PositionSource, ProjectFolder};
use eframe::egui::{Button, Ui};
use thiserror::Error;

use super::{ProjectTab, position_edit};

const MIN_ALIGN_ITEMS: usize = 2;
const MIN_DISTRIBUTE_ITEMS: usize = 3;
const MAX_FINDER_POINT: f64 = u32::MAX as f64;

#[derive(Debug, Error)]
#[error("Position of {0} must use whole folder points between 0 and {MAX_FINDER_POINT}.")]
struct PositionError(VirtualPath);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Arrangement {
	Align(Axis, Alignment),
	Distribute(Axis),
	Nudge([f64; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Axis {
	Horizontal,
	Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Alignment {
	Start,
	Center,
	End,
}

const MENU: [(&str, Arrangement); 8] = [
	(
		"Align left",
		Arrangement::Align(Axis::Horizontal, Alignment::Start),
	),
	(
		"Align horizontal centers",
		Arrangement::Align(Axis::Horizontal, Alignment::Center),
	),
	(
		"Align right",
		Arrangement::Align(Axis::Horizontal, Alignment::End),
	),
	(
		"Align top",
		Arrangement::Align(Axis::Vertical, Alignment::Start),
	),
	(
		"Align vertical centers",
		Arrangement::Align(Axis::Vertical, Alignment::Center),
	),
	(
		"Align bottom",
		Arrangement::Align(Axis::Vertical, Alignment::End),
	),
	(
		"Distribute horizontally",
		Arrangement::Distribute(Axis::Horizontal),
	),
	(
		"Distribute vertically",
		Arrangement::Distribute(Axis::Vertical),
	),
];

impl Axis {
	const fn index(self) -> usize {
		match self {
			Self::Horizontal => 0,
			Self::Vertical => 1,
		}
	}
}

impl Arrangement {
	const fn minimum_items(self) -> usize {
		match self {
			Self::Nudge(_) => 1,
			Self::Align(..) => MIN_ALIGN_ITEMS,
			Self::Distribute(_) => MIN_DISTRIBUTE_ITEMS,
		}
	}

	fn edits(self, items: Vec<&PlacedItem<'_>>) -> Result<Vec<ProjectEdit>, PositionError> {
		if items.len() < self.minimum_items() {
			return Ok(Vec::new());
		}
		let mut positions: Vec<_> = items.iter().map(|item| components(item.center)).collect();
		match self {
			Self::Nudge([dx, dy]) => {
				for [x, y] in &mut positions {
					*x += dx;
					*y += dy;
				}
			}
			Self::Align(axis, aligned) => align(&mut positions, axis, aligned),
			Self::Distribute(axis) => distribute(&mut positions, axis),
		}
		if items
			.iter()
			.zip(&positions)
			.all(|(item, position)| components(item.center) == *position)
		{
			return Ok(Vec::new());
		}
		items
			.into_iter()
			.zip(positions)
			.filter(|(item, position)| {
				// The preview grid reflows when any of its entries gains a stored position.
				components(item.center) != *position
					|| item.position_source == PositionSource::PreviewGrid
			})
			.map(|(item, position)| {
				let position = if item.position_source == PositionSource::PreviewGrid {
					position.map(f64::round)
				} else {
					position
				};
				if position
					.iter()
					.any(|point| !(0.0..=MAX_FINDER_POINT).contains(point) || point.fract() != 0.0)
				{
					return Err(PositionError(item.destination.clone()));
				}
				Ok(position_edit(item.destination.clone(), position))
			})
			.collect()
	}
}

fn components(point: FolderPoint) -> [f64; 2] {
	[point.x(), point.y()]
}

fn align(positions: &mut [[f64; 2]], axis: Axis, aligned: Alignment) {
	let index = axis.index();
	let (start, end) = positions.iter().fold(
		(f64::INFINITY, f64::NEG_INFINITY),
		|(start, end), position| (start.min(position[index]), end.max(position[index])),
	);
	let coordinate = match aligned {
		Alignment::Start => start,
		Alignment::Center => start.midpoint(end),
		Alignment::End => end,
	};
	for position in positions {
		position[index] = coordinate.round();
	}
}

fn distribute(positions: &mut [[f64; 2]], axis: Axis) {
	let index = axis.index();
	let mut order: Vec<_> = (0..positions.len()).collect();
	// Stable ties use the folder's stored-name ordering.
	order.sort_by(|&left, &right| positions[left][index].total_cmp(&positions[right][index]));
	let intervals = order.len() - 1;
	let start = positions[order[0]][index];
	let end = positions[order[intervals]][index];
	for (offset, &item) in order[1..intervals].iter().enumerate() {
		let fraction = (offset + 1) as f64 / intervals as f64;
		positions[item][index] = (start * (1.0 - fraction) + end * fraction).round();
	}
}

pub(super) fn toolbar(ui: &mut Ui, movable_items: usize) -> Option<Arrangement> {
	let mut command = None;
	ui.add_enabled_ui(movable_items >= MIN_ALIGN_ITEMS, |ui| {
		ui.menu_button("Arrange", |ui| {
			for (label, candidate) in MENU {
				if ui
					.add_enabled(
						movable_items >= candidate.minimum_items(),
						Button::new(label),
					)
					.clicked()
				{
					command = Some(candidate);
					ui.close();
				}
			}
		})
		.response
		.on_hover_text("Align selected icons or space them evenly.")
		.on_disabled_hover_text("Select at least two visible entries with editable positions.");
	});
	command
}

pub(super) fn movable_selection<'layout, 'project>(
	layout: &'layout ProjectFolder<'project>,
	project: &Project,
	selected: &BTreeSet<VirtualPath>,
	show_hidden: bool,
) -> Option<Vec<&'layout PlacedItem<'project>>> {
	if selected.is_empty()
		|| project
			.contents
			.iter()
			.filter(|entry| selected.contains(&entry.destination))
			.count() != selected.len()
	{
		return None;
	}
	let items: Vec<_> = layout
		.items()
		.iter()
		.filter(|item| {
			selected.contains(item.destination)
				&& (show_hidden || !item.hidden)
				&& item.position_source != PositionSource::FinderFolder
		})
		.collect();
	(items.len() == selected.len()).then_some(items)
}

impl ProjectTab {
	pub(super) fn arrange(&mut self, command: Arrangement) {
		if self.drag.is_some() {
			return;
		}
		let edits = self.layout().ok().and_then(|layout| {
			let items = movable_selection(
				&layout,
				self.document.project(),
				&self.selected,
				self.show_hidden,
			)?;
			Some(command.edits(items))
		});
		match edits {
			Some(Ok(edits)) => self.apply_edits(&edits),
			Some(Err(error)) => self.error = Some(error.to_string()),
			None => (),
		}
	}
}
