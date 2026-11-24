use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, ContainerFormat, DiskReport, InspectionReport,
	PartitionDetails, PartitionReport, RecognizedFilesystem,
};
use eframe::egui::{self, Grid, Panel, RichText, ScrollArea, Ui};

const SUMMARY_WIDTH: f32 = 264.0;

pub(crate) fn show(ui: &mut Ui, report: &InspectionReport) {
	Panel::left("image_summary")
		.default_size(SUMMARY_WIDTH)
		.show(ui, |ui| {
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
			field(ui, "Structure", status(report.coverage.structure));
			field(ui, "Payload", status(report.coverage.payload));
			field(ui, "Filesystem", status(report.coverage.filesystem));
			field(ui, "Security", status(report.coverage.security));
		});
	egui::CentralPanel::default().show(ui, |ui| {
		ScrollArea::vertical().show(ui, |ui| {
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
			if let Some(page) = &report.run_page {
				ui.separator();
				ui.heading("Runs");
				Grid::new("run_rows").striped(true).show(ui, |ui| {
					for heading in ["Run", "Codec", "Logical start", "Length", "Stored bytes"] {
						ui.strong(heading);
					}
					ui.end_row();
					for (index, run) in (page.offset..).zip(&page.runs) {
						ui.label(index.to_string());
						ui.label(format!("{:?}", run.encoding));
						ui.label(run.logical.offset_bytes.to_string());
						ui.label(run.logical.length_bytes.to_string());
						ui.label(run.physical.length_bytes.to_string());
						ui.end_row();
					}
				});
				if page.next_offset.is_some() {
					ui.small(format!(
						"Showing {} of {} runs",
						page.runs.len(),
						report.container.runs.total
					));
				}
			}
		});
	});
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
		CheckStatus::Failed => "Failed",
		CheckStatus::NotPresent => "Absent",
		CheckStatus::Unsupported => "Unsupported",
		_ => "Unavailable",
	}
}
