use dmgez_hfsplus_read::browse::VolumeSignature;
use dmgez_inspector::{DiskReport, DiskScheme, RecognizedFilesystem};
use dmgez_vfs::{PartitionId, Scheme, VolumeId, volume::NodeKind};
use eframe::egui::{self, Grid, Panel, ScrollArea, Ui};

use crate::image::browse::{Browser, Details, Location, PAGE_ENTRIES, Page, Request, label};

use super::{Action, field};

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
	browser: &Browser,
	disk: Option<&DiskReport>,
	idle: bool,
) -> Option<Action> {
	let request = ui
		.add_enabled_ui(idle, |ui| {
			let variants = volume_choices(disk);
			let selected = browser.page.as_ref().map(|page| page.location.volume);
			ui.horizontal_wrapped(|ui| {
				ui.heading("Files");
				choose_volume(ui, &variants, selected)
			})
			.inner
			.or_else(|| browser.page.as_ref().and_then(|page| show_folder(ui, page)))
		})
		.inner;
	request.map(Action::Browse)
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

fn show_folder(ui: &mut Ui, page: &Page) -> Option<Request> {
	let at_root = page.location.path.is_empty();
	let folder = ui
		.horizontal_wrapped(|ui| {
			ui.label(match page.filesystem {
				VolumeSignature::HfsPlus => "HFS+",
				VolumeSignature::HfsX => "HFSX",
			});
			let mut request = None;
			if ui.add_enabled(!at_root, egui::Button::new("Up")).clicked() {
				request = Some(Request::first(
					page.location.ancestor(page.location.path.len() - 1),
				));
			}
			if ui
				.add_enabled(!at_root, egui::Button::new("Root"))
				.clicked()
			{
				request = Some(Request::first(page.location.ancestor(0)));
			}
			for (index, name) in page.location.path.iter().enumerate() {
				let depth = index + 1;
				ui.label("/");
				let title = label(name);
				if depth == page.location.path.len() {
					ui.strong(title);
				} else if ui.button(title).clicked() {
					request = Some(Request::first(page.location.ancestor(depth)));
				}
			}
			request
		})
		.inner;
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
	let entry = Grid::new("volume_files")
		.striped(true)
		.show(ui, |ui| {
			let mut request = None;
			for (index, entry) in page.entries.iter().enumerate() {
				let name = label(&entry.name);
				if entry.kind == NodeKind::Directory {
					let child = page.location.child(&entry.name);
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
					ui.button("Details")
						.on_hover_text(format!("Inspect {name}"))
				});
				if details.inner.clicked() {
					request = Some(Request::Details {
						folder: page.location.clone(),
						name: entry.name.clone(),
					});
				}
				ui.end_row();
			}
			request
		})
		.inner;
	folder.or(paging).or(entry)
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
}

fn kind_label(kind: NodeKind) -> &'static str {
	match kind {
		NodeKind::Directory => "Folder",
		NodeKind::RegularFile => "File",
		NodeKind::Symlink => "Symbolic link",
		_ => "Other",
	}
}
