use std::path::Path;

use dmgez_diff::{Outcome, Report as ByteReport};
use dmgez_runtime::compare::ByteSpace;
use eframe::egui::Ui;

use crate::image::compare::Report;

use super::file_label;

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod filesystem;

pub(super) const PREFERRED_WIDTH: f32 = 640.0;

pub(super) fn report(
	ui: &mut Ui,
	left: &Path,
	right: &Path,
	report: &Report,
	_selected: &mut Option<usize>,
	idle: bool,
) -> Option<u64> {
	ui.columns(2, |sides| {
		sides[0].strong("Open image");
		sides[0].label(file_label(left));
		sides[1].strong("Other image");
		sides[1].label(file_label(right));
	});
	match report {
		Report::Bytes {
			address_space,
			report,
		} => byte_report(ui, *address_space, report, idle),
		#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
		Report::Filesystem(report) => {
			filesystem::show(ui, report, _selected);
			None
		}
	}
}

fn byte_report(ui: &mut Ui, space: ByteSpace, report: &ByteReport, idle: bool) -> Option<u64> {
	let space_label = match space {
		ByteSpace::Logical(_) => "Logical disk bytes",
		ByteSpace::Physical => "Stored image bytes",
	};
	ui.strong(match report.outcome {
		Outcome::Equal => format!("{space_label} match"),
		Outcome::ByteMismatch => format!(
			"{space_label} differ at byte {}",
			report.common_prefix_bytes
		),
		Outcome::LengthMismatch => {
			format!("{space_label} have different lengths and a matching prefix")
		}
		_ => space_label.into(),
	});
	ui.label(format!(
		"{} matching bytes · Open image {} bytes · Other image {} bytes",
		report.common_prefix_bytes, report.left_bytes, report.right_bytes,
	));
	if matches!(space, ByteSpace::Logical(_))
		&& report.outcome != Outcome::Equal
		&& report.common_prefix_bytes < report.left_bytes
		&& ui
			.add_enabled(idle, eframe::egui::Button::new("Go to first differing run"))
			.clicked()
	{
		Some(report.common_prefix_bytes)
	} else {
		None
	}
}
