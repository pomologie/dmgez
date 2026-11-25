use std::path::{Path, PathBuf};

use dmgez_runtime::image::InspectionExportError;
use dmgez_udif::Cancellation;
use eframe::egui::Context;
use rfd::FileDialog;

use crate::image::{ImageTab, exports::Kind};

use super::{App, Completion, Document, Opened, image_tasks::ImageCompletion};

const JSON_EXTENSION: &str = "json";
const RAW_EXTENSION: &str = "raw";

impl App {
	pub(super) fn start_image_export(&mut self, ctx: &Context, kind: Kind) {
		self.start_image_export_with(ctx, kind, choose_destination);
	}

	fn start_image_export_with(
		&mut self,
		ctx: &Context,
		kind: Kind,
		pick: impl FnOnce(&Path, Kind) -> Option<PathBuf> + Send + 'static,
	) {
		if self.worker.is_some() {
			return;
		}
		let Some(Opened {
			path,
			document: Document::Image(image),
		}) = &mut self.opened
		else {
			return;
		};
		let Some(export) = image.export(kind) else {
			return;
		};
		image.exported = None;
		let source_path = path.clone();
		let cancellation = Cancellation::default();
		self.start_worker(ctx, Some(cancellation.clone()), move || {
			let Some(destination) = pick(&source_path, kind) else {
				return Completion::Cancelled;
			};
			let result = export
				.execute(&destination, &cancellation)
				.map(|()| destination);
			Completion::Image(ImageCompletion::Exported(result))
		});
	}

	pub(super) fn finish_image_export(
		image: &mut ImageTab,
		message: &mut Option<String>,
		result: Result<PathBuf, InspectionExportError>,
	) {
		match result {
			Ok(path) => image.exported = Some(path),
			Err(error) if error.cancelled() => {}
			Err(error) => {
				if error.source_changed() {
					image.verification = None;
					image.run_navigation.select(None);
					#[cfg(feature = "hfsplus-read")]
					image.browser.invalidate();
				}
				*message = Some(format!("Could not export the image data. {error}"));
			}
		}
	}
}

fn choose_destination(path: &Path, kind: Kind) -> Option<PathBuf> {
	let (title, file_name, filter, extension) = match kind {
		Kind::Inspection => (
			"Export inspection report",
			"inspection.json",
			"JSON report",
			JSON_EXTENSION,
		),
		Kind::Verification => (
			"Export verification report",
			"verification.json",
			"JSON report",
			JSON_EXTENSION,
		),
		Kind::Run(_) => (
			"Export selected logical run",
			"run.raw",
			"Raw disk bytes",
			RAW_EXTENSION,
		),
	};
	let dialog = FileDialog::new()
		.set_title(title)
		.set_file_name(file_name)
		.add_filter(filter, &[extension]);
	let dialog = match path.parent() {
		Some(directory) => dialog.set_directory(directory),
		None => dialog,
	};
	dialog.save_file()
}

#[cfg(test)]
mod tests;
