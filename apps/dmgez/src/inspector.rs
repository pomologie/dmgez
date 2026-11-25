use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, ContainerFormat, DiskReport, PartitionDetails,
	PartitionReport, RecognizedFilesystem,
};
use dmgez_runtime::image::RunQuery;
use eframe::egui::{self, Panel, RichText, ScrollArea, Ui};

use crate::image::{ImageTab, exports::Kind as Export};

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod browse;
mod runs;

const SUMMARY_WIDTH: f32 = 264.0;

pub(crate) enum Action {
	Page(usize),
	LocateRun(u64),
	QueryRuns {
		query: RunQuery,
		offset: usize,
	},
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
						runs::show(
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
