use dmgez_hfsplus_read::browse::{Error as HfsError, VolumeSignature};
use dmgez_inspector::{DiskReport, DiskScheme, RecognizedFilesystem};
use dmgez_vfs::{
	PartitionId, Scheme, VolumeId,
	volume::{NodeKind, ViewError},
};
use eframe::egui::{self, Grid, Panel, ScrollArea, Ui};

use crate::image::browse::{
	Browser, Details, Entry, Location, PAGE_ENTRIES, Page, Request,
	history::{Direction, History},
	label,
};

use super::{Action, bytes, field};

#[cfg(feature = "actual-preview")]
mod preview;
mod search;

const VOLUME_MENU_HEIGHT: f32 = 280.0;
const DETAILS_WIDTH: f32 = 220.0;
const FOLDER_CELL: &str = "folder";
const DETAILS_CELL: &str = "details";

pub(super) fn navigation(ui: &mut Ui, browser: &Browser, idle: bool) -> Option<Action> {
	ui.add_enabled_ui(idle, |ui| {
		ui.horizontal(|ui| {
			let container = ui.selectable_label(!browser.showing, "Container");
			let files = ui.selectable_label(browser.showing, "Files");
			if files.clicked() && !browser.showing {
				Some(Action::ShowFiles)
			} else if container.clicked() && browser.showing {
				Some(Action::ShowContainer)
			} else {
				None
			}
		})
		.inner
	})
	.inner
}

pub(super) fn show(
	ui: &mut Ui,
	browser: &mut Browser,
	disk: Option<&DiskReport>,
	idle: bool,
) -> Option<Action> {
	ui.add_enabled_ui(idle, |ui| {
		let variants = volume_choices(disk);
		let selected = browser.page.as_ref().map(|page| page.location.volume);
		ui.horizontal_wrapped(|ui| {
			ui.heading("Files");
			choose_volume(ui, &variants, selected)
		})
		.inner
		.map(Action::Browse)
		.or_else(|| {
			let page = browser.page.as_ref()?;
			let navigation = folder_navigation(ui, page, &browser.history);
			let (results_visible, search) = search::show(ui, &page.location, &mut browser.search);
			let navigation = navigation.or(search.map(Action::Browse));
			if results_visible {
				return navigation;
			}
			#[cfg(feature = "actual-preview")]
			let navigation = {
				let action = preview::show(ui, &page.location, browser.preview.as_ref());
				if browser.preview.is_some() {
					return navigation.or(action);
				}
				navigation.or(action)
			};
			navigation.or_else(|| show_folder_entries(ui, page).map(Action::Browse))
		})
	})
	.inner
}

fn choose_volume(
	ui: &mut Ui,
	variants: &[VolumeId],
	selected: Option<VolumeId>,
) -> Option<Request> {
	match variants {
		[] => {
			ui.label("No HFS+ or HFSX data window is available to try.");
			None
		}
		[volume] if selected.is_none() => ui
			.button(format!("Open {}", volume_label(*volume)))
			.on_hover_text("Check this data window for an HFS+ or HFSX volume")
			.clicked()
			.then(|| Request::first(Location::root(*volume))),
		[_] => None,
		_ => ui
			.menu_button(
				selected.map_or_else(|| "Choose partition…".into(), volume_label),
				|ui| {
					ScrollArea::vertical()
						.max_height(VOLUME_MENU_HEIGHT)
						.show(ui, |ui| {
							let mut request = None;
							for volume in variants {
								if ui
									.selectable_label(
										Some(*volume) == selected,
										volume_label(*volume),
									)
									.on_hover_text(
										"Check this partition for an HFS+ or HFSX volume",
									)
									.clicked()
								{
									request = Some(Request::first(Location::root(*volume)));
									ui.close();
								}
							}
							request
						})
						.inner
				},
			)
			.inner
			.flatten(),
	}
}

fn volume_choices(disk: Option<&DiskReport>) -> Vec<VolumeId> {
	match disk {
		Some(DiskReport::Partitionless {
			filesystem: RecognizedFilesystem::HfsPlus | RecognizedFilesystem::HfsX,
			..
		}) => vec![VolumeId::WholeDisk],
		Some(DiskReport::Partitioned {
			scheme, partitions, ..
		}) => {
			let scheme = match scheme {
				DiskScheme::Apm => Scheme::Apple,
				DiskScheme::Gpt => Scheme::Gpt,
				DiskScheme::Mbr => Scheme::Mbr,
				_ => return Vec::new(),
			};
			partitions
				.iter()
				.filter(|partition| partition.logical_data.is_some())
				.map(|partition| {
					VolumeId::Partition(PartitionId {
						scheme,
						slot: partition.id.slot,
					})
				})
				.collect()
		}
		_ => Vec::new(),
	}
}

fn volume_label(id: VolumeId) -> String {
	match id {
		VolumeId::WholeDisk => "whole-disk volume".into(),
		VolumeId::Partition(id) => format!("{} partition {}", id.scheme, id.slot),
		_ => "unknown volume".into(),
	}
}

fn folder_navigation(ui: &mut Ui, page: &Page, history: &History) -> Option<Action> {
	let at_root = page.location.path.is_empty();
	ui.horizontal_wrapped(|ui| {
		ui.label(match page.filesystem {
			VolumeSignature::HfsPlus => "HFS+",
			VolumeSignature::HfsX => "HFSX",
		});
		let mut action = None;
		for (direction, title) in [(Direction::Back, "Back"), (Direction::Forward, "Forward")] {
			if ui
				.add_enabled(history.available(direction), egui::Button::new(title))
				.clicked()
			{
				action = Some(Action::History(direction));
			}
		}
		if ui.add_enabled(!at_root, egui::Button::new("Up")).clicked() {
			action = Some(Action::Browse(Request::first(
				page.location.ancestor(page.location.path.len() - 1),
			)));
		}
		if ui
			.add_enabled(!at_root, egui::Button::new("Root"))
			.clicked()
		{
			action = Some(Action::Browse(Request::first(page.location.ancestor(0))));
		}
		for (index, name) in page.location.path.iter().enumerate() {
			let depth = index + 1;
			ui.label("/");
			let title = label(name);
			if depth == page.location.path.len() {
				ui.strong(title);
			} else if ui.button(title).clicked() {
				action = Some(Action::Browse(Request::first(
					page.location.ancestor(depth),
				)));
			}
		}
		action
	})
	.inner
}

fn show_folder_entries(ui: &mut Ui, page: &Page) -> Option<Request> {
	let paging = ui
		.horizontal(|ui| {
			let previous = ui.add_enabled(page.offset != 0, egui::Button::new("Previous files"));
			let next = ui.add_enabled(page.next.is_some(), egui::Button::new("Next files"));
			if !page.entries.is_empty() {
				let first = page.offset + 1;
				let last = page.offset + page.entries.len() as u64;
				ui.label(format!("{first}–{last}"));
			}
			if previous.clicked() {
				Some(Request::Page {
					location: page.location.clone(),
					offset: page.offset.saturating_sub(PAGE_ENTRIES),
				})
			} else if next.clicked() {
				page.next.map(|offset| Request::Page {
					location: page.location.clone(),
					offset,
				})
			} else {
				None
			}
		})
		.inner;
	if page.entries.is_empty() {
		ui.label("No entries in this page.");
	}
	if page.truncated {
		ui.label("More entries exist beyond the directory browsing limit.");
	}
	paging.or(show_entries(ui, &page.location, &page.entries))
}

fn show_entries(ui: &mut Ui, location: &Location, entries: &[Entry]) -> Option<Request> {
	Grid::new("volume_files")
		.striped(true)
		.show(ui, |ui| {
			let mut request = None;
			for (index, entry) in entries.iter().enumerate() {
				let name = label(&entry.name);
				if entry.kind == NodeKind::Directory {
					let child = location.child(&entry.name);
					let open = ui.push_id((index, FOLDER_CELL), |ui| {
						ui.add_enabled(child.is_some(), egui::Button::new(name.as_str()))
					});
					if open.inner.clicked() {
						request = child.map(Request::first);
					}
				} else {
					ui.label(name.as_str());
				}
				ui.label(kind_label(entry.kind));
				let details = ui.push_id((index, DETAILS_CELL), |ui| {
					let control = ui
						.button("Details")
						.on_hover_text(format!("Inspect {name}"));
					control.widget_info(|| {
						egui::WidgetInfo::labeled(
							egui::Role::Button,
							control.enabled(),
							format!("Details for {name}"),
						)
					});
					control
				});
				if details.inner.clicked() {
					request = Some(Request::Details {
						folder: location.clone(),
						name: entry.name.clone(),
					});
				}
				ui.end_row();
			}
			request
		})
		.inner
}

pub(super) fn details_panel(ui: &mut Ui, details: &Details) {
	Panel::right("volume_entry_details")
		.default_size(DETAILS_WIDTH)
		.show(ui, |ui| {
			ScrollArea::vertical().show(ui, |ui| show_details(ui, details));
		});
}

fn show_details(ui: &mut Ui, details: &Details) {
	ui.heading("Item details");
	ui.strong(label(&details.name));
	let metadata = &details.metadata;
	field(ui, "Kind", kind_label(metadata.kind));
	field(ui, "Mode", format!("{:06o}", metadata.mode));
	field(ui, "Owner ID", metadata.uid.to_string());
	field(ui, "Group ID", metadata.gid.to_string());
	if let Some(id) = details.native_id {
		field(ui, "Catalog ID", id.to_string());
	}
	if let Some(hard_link) = &metadata.hard_link {
		field(ui, "Hard links", hard_link.count.to_string());
		field(
			ui,
			"Backing catalog ID",
			hard_link.storage.native_id().to_string(),
		);
	}
	if let Some(target) = &details.link_target {
		let escaped: String = target
			.iter()
			.flat_map(|&byte| std::ascii::escape_default(byte))
			.map(char::from)
			.collect();
		field(ui, "Stored link target", escaped);
	}
	if let Some(forks) = &details.storage.forks {
		show_fork_size(ui, "Data fork", &forks.data);
		show_fork_size(ui, "Resource fork", &forks.resource);
	}
	ui.separator();
	ui.strong("Extended attributes");
	match &details.storage.xattrs {
		Ok(Some(names)) if names.is_empty() => {
			ui.label("None recorded");
		}
		Ok(Some(names)) => {
			for name in names {
				ui.label(label(name));
			}
		}
		Ok(None) => {
			ui.label("Unavailable");
		}
		Err(error) => {
			ui.label(storage_error(error));
		}
	}
}

fn show_fork_size(ui: &mut Ui, title: &str, size: &Result<Option<u64>, ViewError<HfsError>>) {
	let value = match size {
		Ok(Some(length)) => bytes(*length),
		Ok(None) => "Absent".into(),
		Err(error) => storage_error(error),
	};
	field(ui, title, value);
}

fn storage_error(error: &ViewError<HfsError>) -> String {
	match error {
		ViewError::Backend(cause) => format!("Unavailable ({cause})"),
		_ => format!("Unavailable ({error})"),
	}
}

fn kind_label(kind: NodeKind) -> &'static str {
	match kind {
		NodeKind::Directory => "Folder",
		NodeKind::RegularFile => "File",
		NodeKind::Symlink => "Symbolic link",
		_ => "Other",
	}
}
