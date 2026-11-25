use dmgez_inspector::DiskReport;
use dmgez_vfs::VolumeId;
use eframe::egui::{ScrollArea, Ui};

use crate::image::browse::{Browser, Request};

use super::{VOLUME_MENU_HEIGHT, volume_label};

const PROBE_WHOLE_DISK: &str = "Check whole disk for APFS";

pub(super) fn show(
	ui: &mut Ui,
	disk: Option<&DiskReport>,
	windows: &[VolumeId],
	browser: &Browser,
) -> Option<Request> {
	let probe = if matches!(disk, Some(DiskReport::Unknown)) {
		ui.button(PROBE_WHOLE_DISK)
			.clicked()
			.then_some(Request::ApfsCatalog(VolumeId::WholeDisk))
	} else if !windows.is_empty() {
		ui.menu_button("Check partition for APFS…", |ui| {
			ScrollArea::vertical()
				.max_height(VOLUME_MENU_HEIGHT)
				.show(ui, |ui| {
					windows.iter().find_map(|&window| {
						if ui.button(volume_label(window)).clicked() {
							ui.close();
							Some(Request::ApfsCatalog(window))
						} else {
							None
						}
					})
				})
				.inner
		})
		.inner
		.flatten()
	} else {
		None
	};
	probe.or_else(|| {
		let catalog = browser.apfs.as_ref()?;
		ui.label(format!(
			"APFS volumes in {} at XID {}",
			volume_label(catalog.window),
			catalog.checkpoint.0
		));
		if !catalog.diagnostics.is_empty() {
			ui.collapsing("Latest APFS checkpoint could not be established", |ui| {
				ui.label("Choose a volume to inspect this retained checkpoint.");
				for diagnostic in &catalog.diagnostics {
					ui.label(format!("{diagnostic:?}"));
				}
			});
		}
		if catalog.volumes.is_empty() {
			ui.label("No APFS volumes found at this checkpoint.");
		}
		ScrollArea::vertical()
			.max_height(VOLUME_MENU_HEIGHT)
			.show(ui, |ui| {
				catalog.volumes.iter().find_map(|info| {
					let label = format!(
						"Open {} (object {})",
						info.name.as_str().escape_debug(),
						info.object_id.0
					);
					ui.button(label)
						.clicked()
						.then(|| Request::first(catalog.root(info)))
				})
			})
			.inner
	})
}
