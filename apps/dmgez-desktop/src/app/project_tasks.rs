use std::{
	path::{Path, PathBuf},
	sync::{Arc, Mutex},
};

use dmgez_config::{ImageFormat, project_edit::RevisionId};
use dmgez_runtime::project_build::{
	BuildError, BuildEvent, BuildMode, BuildOutcome, BuildProgress, BuildProject,
};
use dmgez_udif::Cancellation;
use eframe::egui::{Context, ProgressBar, Ui};

use super::{App, Completion, Document, ERROR, IMAGE_EXTENSION, Opened, Pending, document};

const OUTPUT_NAME: &str = "image.dmg";

type Progress = Arc<Mutex<Option<BuildProgress>>>;

#[derive(Default)]
pub(super) struct BuildPanel {
	progress: Option<Progress>,
	completed: Option<BuildCompletion>,
}

pub(super) struct BuildCompletion {
	revision: RevisionId,
	unsaved: bool,
	outcome: Result<Option<BuildOutcome>, BuildError>,
}

impl BuildPanel {
	pub(super) fn finish(&mut self, completion: BuildCompletion) {
		self.clear_progress();
		self.completed = Some(completion);
	}

	pub(super) fn clear_progress(&mut self) {
		self.progress = None;
	}
}

impl App {
	pub(super) fn build_actions(&mut self, ui: &mut Ui) {
		if ui.button("Plan build").clicked() {
			self.build_from_editor(ui.ctx(), BuildMode::Plan);
		}
		if ui.button("Build image…").clicked() {
			self.build_from_editor(ui.ctx(), BuildMode::Publish);
		}
	}

	pub(super) fn build_from_editor(&mut self, ctx: &Context, mode: BuildMode) {
		self.start_project_build(ctx, mode, choose_output);
	}

	fn start_project_build(
		&mut self,
		ctx: &Context,
		mode: BuildMode,
		pick: impl FnOnce(&Path) -> Option<PathBuf> + Send + 'static,
	) {
		if self.worker.is_some() {
			return;
		}
		let Some(Opened {
			path,
			document: Document::Project(project),
		}) = &self.opened
		else {
			return;
		};
		let Some(directory) = path.parent() else {
			return;
		};
		let directory = directory.to_owned();
		let hint = project.document.project().image.output.resolve(&directory);
		let revision = project.document.revision();
		let unsaved = project.is_dirty();
		let source = project.document.source().to_owned();
		let cancellation = Cancellation::default();
		let progress = Progress::default();
		let updates = progress.clone();
		let repaint = ctx.clone();
		self.start_worker(ctx, Some(cancellation.clone()), move || {
			let outcome = (|| {
				if cancellation.is_cancelled() {
					return Ok(None);
				}
				let destination = match mode {
					BuildMode::Plan => None,
					BuildMode::Publish => match pick(&hint) {
						Some(path) => Some(path),
						None => return Ok(None),
					},
				};
				if cancellation.is_cancelled() {
					return Ok(None);
				}
				let build =
					BuildProject::from_snapshot(&source, &directory, destination.as_deref())?;
				build
					.run(mode, cancellation, |event| {
						if let BuildEvent::Progress(stage) = event
							&& let Ok(mut update) = updates.lock()
						{
							*update = Some(stage);
							repaint.request_repaint();
						}
					})
					.map(Some)
			})();
			Completion::Build(BuildCompletion {
				revision,
				unsaved,
				outcome,
			})
		});
		if self.worker.is_some() {
			self.build = BuildPanel {
				progress: Some(progress),
				completed: None,
			};
		}
	}

	pub(super) fn show_build(&mut self, ui: &mut Ui) {
		if let Some(progress) = &self.build.progress {
			let stage = progress.try_lock().ok().and_then(|update| *update);
			if let Some(stage) = stage {
				show_progress(ui, stage);
			}
		}
		let Some(completed) = &self.build.completed else {
			return;
		};
		let open = completed.show(ui, self.worker.is_none());
		if let Some(Opened {
			document: Document::Project(project),
			..
		}) = &self.opened
			&& completed.revision != project.document.revision()
		{
			ui.weak("The project changed after this build started.");
		}
		if let Some(path) = open {
			if self.is_dirty() {
				self.confirm = Some(Pending::OpenBuiltImage(path));
			} else {
				self.start(ui.ctx(), move || open_built_outcome(path));
			}
		}
	}
}

impl BuildCompletion {
	fn show(&self, ui: &mut Ui, idle: bool) -> Option<PathBuf> {
		let mut open = None;
		ui.horizontal_wrapped(|ui| match &self.outcome {
			Ok(Some(BuildOutcome::Planned(plan))) => {
				ui.strong("Build plan");
				ui.label(format!(
					"{:?} · {} entries · {} logical bytes",
					plan.format, plan.resolved_entries, plan.logical_bytes
				));
				if let Some(path) = &plan.output {
					ui.label(path.display().to_string());
				}
				show_losses(ui, plan.metadata_losses);
			}
			Ok(Some(BuildOutcome::Published {
				output,
				format,
				report,
				metadata_losses,
			})) => {
				ui.strong("Unsigned image built");
				ui.label(format!("{} bytes", report.image.physical_bytes));
				if report.verified {
					ui.label("Verified before publication");
				}
				show_losses(ui, *metadata_losses);
				if let Some(path) = output {
					ui.label(path.display().to_string());
					if *format != ImageFormat::Udrw
						&& ui
							.add_enabled(idle, eframe::egui::Button::new("Open image"))
							.clicked()
					{
						open = Some(path.clone());
					}
				}
			}
			Ok(None) => {
				ui.label("Build cancelled.");
			}
			Err(error) => {
				ui.colored_label(ERROR, error.to_string());
			}
		});
		if self.unsaved && matches!(&self.outcome, Ok(Some(_))) {
			ui.weak("Includes unsaved project changes.");
		}
		open
	}
}

pub(super) fn open_built_outcome(path: PathBuf) -> Completion {
	match document::open_built_image(path) {
		Ok(opened) => Completion::Opened(opened),
		Err(error) => Completion::Failed(error),
	}
}

fn choose_output(hint: &Path) -> Option<PathBuf> {
	let dialog = rfd::FileDialog::new()
		.set_title("Build a new disk image")
		.add_filter("Disk image", &[IMAGE_EXTENSION])
		.set_file_name(
			hint.file_name()
				.and_then(|name| name.to_str())
				.unwrap_or(OUTPUT_NAME),
		);
	let dialog = match hint.parent() {
		Some(directory) => dialog.set_directory(directory),
		None => dialog,
	};
	dialog.save_file()
}

fn show_progress(ui: &mut Ui, stage: BuildProgress) {
	match stage {
		BuildProgress::Discovering => {
			ui.label("Discovering source files…");
		}
		BuildProgress::SourcesDiscovered {
			entries,
			regular_payload_bytes,
		} => {
			ui.label(format!(
				"Discovered {entries} entries · {regular_payload_bytes} source bytes"
			));
		}
		BuildProgress::FinderAssetsPrepared { assets } => {
			ui.label(format!("Prepared {assets} Finder assets"));
		}
		BuildProgress::WritingImage { logical_bytes } => {
			ui.label(format!("Encoding {logical_bytes} logical bytes…"));
		}
		BuildProgress::ReadingImage {
			logical_bytes_read,
			logical_bytes,
		} => {
			let fraction = logical_bytes_read as f32 / logical_bytes.max(1) as f32;
			ui.add(ProgressBar::new(fraction).text(format!(
				"Read {logical_bytes_read} of {logical_bytes} logical bytes"
			)));
		}
		BuildProgress::VerifyingOutput => {
			ui.label("Verifying the staged image…");
		}
		BuildProgress::Publishing => {
			ui.label("Publishing the image…");
		}
		BuildProgress::Completed { physical_bytes } => {
			ui.label(format!("Published {physical_bytes} bytes"));
		}
		_ => {}
	}
}

fn show_losses(ui: &mut Ui, losses: usize) {
	if losses != 0 {
		ui.label(format!("{losses} metadata losses"));
	}
}

#[cfg(test)]
mod tests;
