use std::{
	error::Error as StdError,
	path::{Path, PathBuf},
};

use dmgez_udif::Cancellation;
use eframe::egui::{self, Context, Ui};

use crate::{
	document::IMAGE_EXTENSION,
	image::compare::{Error, Report, Request},
	inspector,
};

use super::{App, Completion, Document, Opened, display_name};

mod settings;
#[cfg(test)]
mod tests;
mod view;

#[derive(Default)]
pub(super) struct ComparePanel {
	open: bool,
	settings: settings::Settings,
	result: Option<Result<Completed, Error>>,
	selected_change: Option<usize>,
}

pub(super) struct Completed {
	right: PathBuf,
	report: Report,
}

impl App {
	pub(super) fn compare_action(&mut self, ui: &mut Ui) {
		if matches!(
			self.opened,
			Some(Opened {
				document: Document::Image(_),
				..
			})
		) && ui.button("Compare…").clicked()
		{
			self.comparison.open = true;
		}
	}

	pub(super) fn show_comparison(&mut self, ctx: &Context) {
		let Some(Opened {
			path,
			document: Document::Image(_),
		}) = &self.opened
		else {
			return;
		};
		let idle = self.worker.is_none() && self.confirm.is_none();
		match self.comparison.show(ctx, path, idle) {
			Some(PanelAction::Choose(request)) => {
				self.start_comparison(ctx, request, choose_image);
			}
			Some(PanelAction::Locate(offset)) => {
				self.comparison.open = false;
				#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
				self.image_action(ctx, inspector::Action::ShowContainer);
				self.image_action(ctx, inspector::Action::LocateRun(offset));
			}
			None => {}
		}
	}

	fn start_comparison(
		&mut self,
		ctx: &Context,
		request: Request,
		pick: impl FnOnce(&Path) -> Option<PathBuf> + Send + 'static,
	) {
		if self.worker.is_some() {
			return;
		}
		let Some(Opened {
			path,
			document: Document::Image(image),
		}) = &self.opened
		else {
			return;
		};
		let source_path = path.clone();
		let source = image.snapshot.clone();
		let cancellation = Cancellation::default();
		self.comparison.result = None;
		self.comparison.selected_change = None;
		self.start_worker(ctx, Some(cancellation.clone()), move || {
			let Some(right) = pick(&source_path).filter(|_| !cancellation.is_cancelled()) else {
				return Completion::Cancelled;
			};
			let result = source
				.compare_file(&right, request, &cancellation)
				.map(|report| Completed { right, report });
			Completion::Compared(result)
		});
	}

	pub(super) fn finish_comparison(&mut self, result: Result<Completed, Error>) {
		self.comparison.result = Some(result);
	}
}

enum PanelAction {
	Choose(Request),
	Locate(u64),
}

impl ComparePanel {
	fn show(&mut self, ctx: &Context, left: &Path, idle: bool) -> Option<PanelAction> {
		egui::Window::new("Compare disk images")
			.open(&mut self.open)
			.default_width(view::PREFERRED_WIDTH)
			.show(ctx, |ui| {
				let request = ui.add_enabled_ui(idle, |ui| self.settings.show(ui)).inner;
				ui.separator();
				match &self.result {
					Some(Ok(completed)) => view::report(
						ui,
						left,
						&completed.right,
						&completed.report,
						&mut self.selected_change,
						idle,
					)
					.map(PanelAction::Locate),
					Some(Err(error)) => {
						ui.strong("Comparison incomplete");
						ui.label(error.to_string());
						let mut cause = error.source();
						while let Some(error) = cause {
							ui.label(error.to_string());
							cause = error.source();
						}
						None
					}
					None => None,
				}
				.or_else(|| request.map(PanelAction::Choose))
			})
			.and_then(|response| response.inner.flatten())
	}
}

fn choose_image(left: &Path) -> Option<PathBuf> {
	let picker = rfd::FileDialog::new()
		.set_title("Choose an image to compare")
		.add_filter("Disk image", &[IMAGE_EXTENSION]);
	match left.parent() {
		Some(directory) => picker.set_directory(directory),
		None => picker,
	}
	.pick_file()
}

fn file_label(path: &Path) -> String {
	display_name(path).escape_debug().to_string()
}
