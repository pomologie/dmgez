use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, ContainerFormat, DiskReport, PartitionDetails,
	PartitionReport, RecognizedFilesystem, RunPage,
};
use eframe::egui::{self, Align, Grid, Key, Panel, RichText, ScrollArea, TextEdit, Ui};

use crate::image::{
	ImageTab, RUN_PAGE_SIZE, RunNavigation,
	exports::{Kind as Export, selected_run},
};

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod browse;

const SUMMARY_WIDTH: f32 = 264.0;
const MAX_OFFSET_TEXT: usize = u64::MAX.ilog10() as usize + 1;
const HEX_RADIX: u32 = 16;

pub(crate) enum Action {
	Page(usize),
	LocateRun(u64),
	Verify,
	Export(Export),
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	ShowContainer,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	ShowFiles,
	#[cfg(feature = "actual-preview")]
	ShowList,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	History(crate::image::browse::history::Direction),
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	Browse(crate::image::browse::Request),
}

pub(crate) fn show(ui: &mut Ui, image: &mut ImageTab, idle: bool) -> Option<Action> {
	let report = &image.report;
	let verify = Panel::left("image_summary")
		.default_size(SUMMARY_WIDTH)
		.show(ui, |ui| {
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			let navigation = browse::navigation(ui, &image.browser, idle);
			#[cfg(not(any(feature = "hfsplus-read", feature = "apfs-read")))]
			let navigation = None;
			let container = &report.container;
			ui.heading("Disk image");
			ui.strong(match container.format {
				ContainerFormat::UdifV4 => "UDIF v4",
				_ => "Unknown container",
			});
			ui.add_space(8.0);
			field(ui, "Stored size", bytes(container.physical_bytes));
			field(ui, "Logical disk", bytes(container.logical_bytes));
			field(ui, "Sector size", bytes(container.sector_bytes));
			field(ui, "Block maps", container.block_map_count.to_string());
			if let Some(signature) = container.embedded_signature {
				field(ui, "Embedded signature", bytes(signature.length_bytes));
			}
			field(ui, "Runs", container.runs.total.to_string());
			if container.runs.decode_unavailable != 0 {
				field(
					ui,
					"Unavailable codecs",
					container.runs.decode_unavailable.to_string(),
				);
			}
			ui.separator();
			ui.strong("Verification");
			let coverage = image
				.verification
				.as_ref()
				.map_or(report.coverage, |check| check.coverage);
			field(ui, "Structure", status(coverage.structure));
			field(ui, "Payload", status(coverage.payload));
			field(ui, "Filesystem", status(coverage.filesystem));
			field(ui, "Security", status(coverage.security));
			if let Some(cause) = image.verification.as_ref().and_then(|check| check.cause()) {
				ui.label(cause.to_string());
			}
			let verify = ui
				.add_enabled(idle, egui::Button::new("Verify container"))
				.clicked()
				.then_some(Action::Verify);
			let inspection = ui
				.add_enabled(idle, egui::Button::new("Export inspection JSON…"))
				.clicked()
				.then_some(Action::Export(Export::Inspection));
			let verification = ui
				.add_enabled(
					idle && image.verification.is_some(),
					egui::Button::new("Export verification JSON…"),
				)
				.clicked()
				.then_some(Action::Export(Export::Verification));
			if let Some(path) = &image.exported {
				field(
					ui,
					"Exported",
					path.to_string_lossy().escape_debug().to_string(),
				);
			}
			navigation.or(verify).or(inspection).or(verification)
		})
		.inner;
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	let details = image
		.browser
		.details
		.as_ref()
		.filter(|_| image.browser.showing)
		.and_then(|details| browse::details_panel(ui, details, idle));
	#[cfg(not(any(feature = "hfsplus-read", feature = "apfs-read")))]
	let details = None;
	let page = egui::CentralPanel::default()
		.show(ui, |ui| {
			ScrollArea::both()
				.show(ui, |ui| {
					#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
					if image.browser.showing {
						return browse::show(ui, &mut image.browser, report.disk.as_ref(), idle);
					}
					show_disk(ui, report.disk.as_ref());
					ui.separator();
					ui.heading("Container");
					field(ui, "Segment ID", report.container.segment_id.to_string());
					field(
						ui,
						"Declared data checksum",
						checksum(report.container.data_fork_checksum),
					);
					field(
						ui,
						"Declared master checksum",
						checksum(report.container.master_checksum),
					);
					report.run_page.as_ref().and_then(|page| {
						show_runs(
							ui,
							page,
							report.container.runs.total,
							&mut image.run_navigation,
							idle,
						)
					})
				})
				.inner
		})
		.inner;
	verify.or(details).or(page)
}

fn show_runs(
	ui: &mut Ui,
	page: &RunPage,
	total: usize,
	navigation: &mut RunNavigation,
	idle: bool,
) -> Option<Action> {
	ui.separator();
	ui.heading("Runs");
	if page.runs.is_empty() {
		ui.label("No runs in this page");
		return None;
	}
	let locate = show_run_lookup(ui, navigation, idle);
	let mut export = None;
	if let Some(selected) = navigation.selected {
		ui.horizontal_wrapped(|ui| {
			ui.label(format!("Selected run {}", selected + 1));
			let exportable = selected_run(page, Some(selected));
			if ui
				.add_enabled(
					idle && exportable.is_some(),
					egui::Button::new("Export selected run…"),
				)
				.clicked()
			{
				export = exportable.map(|ordinal| Action::Export(Export::Run(ordinal)));
			}
		});
	}
	let reveal_selected = std::mem::take(&mut navigation.reveal_selected);
	let first = page.offset + 1;
	let last = page.offset + page.runs.len() as u64;
	let offset = usize::try_from(page.offset).ok()?;
	let current_page = offset / RUN_PAGE_SIZE + 1;
	let page_count = total.div_ceil(RUN_PAGE_SIZE);
	let action = ui
		.horizontal_wrapped(|ui| {
			ui.label(format!("{first}–{last} of {total}"));
			let mut requested_page = current_page;
			let previous = ui
				.add_enabled(idle && page.offset > 0, egui::Button::new("Previous page"))
				.clicked();
			let next = ui
				.add_enabled(
					idle && page.next_offset.is_some(),
					egui::Button::new("Next page"),
				)
				.clicked();
			if page_count > 1 {
				ui.add_enabled(
					idle,
					egui::DragValue::new(&mut requested_page)
						.range(1..=page_count)
						.prefix("Page ")
						.update_while_editing(false),
				);
			}
			if previous {
				Some(Action::Page(offset.saturating_sub(RUN_PAGE_SIZE)))
			} else if next {
				page.next_offset.map(Action::Page)
			} else if requested_page != current_page {
				Some(Action::Page((requested_page - 1) * RUN_PAGE_SIZE))
			} else {
				None
			}
		})
		.inner;
	Grid::new("run_rows").striped(true).show(ui, |ui| {
		for heading in [
			"Run",
			"Codec",
			"Logical start",
			"Logical bytes",
			"Stored start",
			"Stored bytes",
		] {
			ui.strong(heading);
		}
		ui.end_row();
		for (ordinal, run) in (offset..).zip(&page.runs) {
			let label = (ordinal + 1).to_string();
			if navigation.selected == Some(ordinal) {
				let response = ui.strong(label);
				if reveal_selected {
					response.scroll_to_me(Some(Align::Center));
				}
			} else {
				ui.label(label);
			}
			ui.label(format!("{:?}", run.encoding));
			ui.label(run.logical.offset_bytes.to_string());
			ui.label(run.logical.length_bytes.to_string());
			if run.physical.length_bytes == 0 {
				ui.label("—");
			} else {
				ui.label(run.physical.offset_bytes.to_string());
			}
			ui.label(run.physical.length_bytes.to_string());
			ui.end_row();
		}
	});
	locate.or(action).or(export)
}

fn show_run_lookup(ui: &mut Ui, navigation: &mut RunNavigation, idle: bool) -> Option<Action> {
	let requested = ui
		.horizontal_wrapped(|ui| {
			let label = ui.label("Logical byte offset");
			let input = ui
				.add_enabled(
					idle,
					TextEdit::singleline(&mut navigation.offset)
						.hint_text("Decimal or 0x…")
						.desired_width(180.0)
						.char_limit(MAX_OFFSET_TEXT + 1),
				)
				.labelled_by(label.id);
			if input.changed() {
				navigation.invalid_offset = false;
			}
			let clicked = ui
				.add_enabled(idle, egui::Button::new("Go to run"))
				.clicked();
			clicked
				|| (idle && input.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
		})
		.inner;
	let action = if requested {
		let offset = parse_logical_offset(&navigation.offset);
		navigation.invalid_offset = offset.is_none();
		offset.map(Action::LocateRun)
	} else {
		None
	};
	if navigation.invalid_offset {
		ui.label("Enter a decimal byte offset or a hexadecimal offset starting with 0x.");
	}
	action
}

fn parse_logical_offset(input: &str) -> Option<u64> {
	// Keep a character beyond the representable input so truncated paste is rejected.
	if input.len() > MAX_OFFSET_TEXT {
		return None;
	}
	let value = input.trim();
	if let Some(hex) = value
		.strip_prefix('0')
		.and_then(|rest| rest.strip_prefix(['x', 'X']))
	{
		u64::from_str_radix(hex, HEX_RADIX).ok()
	} else {
		value.parse().ok()
	}
}

fn show_disk(ui: &mut Ui, disk: Option<&DiskReport>) {
	ui.heading("Logical disk");
	match disk {
		Some(DiskReport::Partitioned {
			scheme,
			logical_block_bytes,
			partitions,
		}) => {
			field(ui, "Partition map", scheme.as_str().to_ascii_uppercase());
			field(ui, "Logical block size", bytes(*logical_block_bytes));
			ui.add_space(8.0);
			for partition in partitions {
				show_partition(ui, partition);
			}
		}
		Some(DiskReport::Partitionless {
			filesystem,
			logical,
		}) => {
			ui.label("Partitionless disk");
			field(
				ui,
				"Detected filesystem",
				match filesystem {
					RecognizedFilesystem::HfsPlus => "HFS+",
					RecognizedFilesystem::HfsX => "HFSX",
					_ => "Unknown",
				},
			);
			field(ui, "Length", bytes(logical.length_bytes));
		}
		Some(DiskReport::Unresolved { diagnostic }) => {
			ui.label(format!(
				"Partition discovery is unresolved ({:?}).",
				diagnostic.category
			));
		}
		Some(DiskReport::Unknown) => {
			ui.label("No recognized partition map or whole-disk filesystem.");
		}
		_ => {
			ui.label("No partition report.");
		}
	}
}

fn show_partition(ui: &mut Ui, part: &PartitionReport) {
	let identity = format!("Partition {}", part.id.slot);
	ui.collapsing(identity, |ui| {
		field(ui, "Role", format!("{:?}", part.role));
		field(ui, "Logical start", bytes(part.logical.offset_bytes));
		field(ui, "Partition length", bytes(part.logical.length_bytes));
		match &part.details {
			PartitionDetails::Apm {
				name, type_code, ..
			} => {
				field(ui, "Name", name);
				field(ui, "Type", type_code);
			}
			PartitionDetails::Gpt {
				name, type_guid, ..
			} => {
				field(ui, "Name", name);
				field(ui, "Type GUID", type_guid.to_string());
			}
			PartitionDetails::Mbr {
				type_code,
				bootable,
				..
			} => {
				field(ui, "Type", format!("{type_code:#04x}"));
				field(ui, "Bootable", if *bootable { "Yes" } else { "No" });
			}
			_ => {}
		}
	});
}

fn field(ui: &mut Ui, label: &str, value: impl Into<String>) {
	let value = value.into();
	ui.horizontal_wrapped(|ui| {
		ui.label(RichText::new(label).weak());
		ui.label(value);
	});
}

fn bytes(amount: u64) -> String {
	format!("{amount} bytes")
}

fn checksum(value: ChecksumDeclaration) -> String {
	match value {
		ChecksumDeclaration::Absent => "Absent".into(),
		ChecksumDeclaration::Crc32 { value } => format!("CRC32 {value:08x}"),
		_ => "Unsupported".into(),
	}
}

fn status(value: CheckStatus) -> &'static str {
	match value {
		CheckStatus::NotChecked => "Not checked",
		CheckStatus::Passed => "Passed",
		CheckStatus::Partial => "Partial",
		CheckStatus::Failed => "Failed",
		CheckStatus::NotPresent => "Absent",
		CheckStatus::Unsupported => "Unsupported",
		_ => "Unavailable",
	}
}
