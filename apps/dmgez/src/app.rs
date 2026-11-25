use std::{
	path::{Path, PathBuf},
	sync::mpsc::TryRecvError,
};

use dmgez_config::project_edit::{ProjectEdit, RevisionId};
use dmgez_runtime::project_save::{Error as ProjectSaveError, FileRevision};
use dmgez_udif::Cancellation;
use eframe::egui::{
	self, Color32, Context, Id, Key, Modifiers, Panel, RichText, Ui, ViewportCommand,
};

use crate::document::{self, Document, IMAGE_EXTENSION, Opened, PROJECT_EXTENSION};
use crate::{import, inspector};

mod image_compare;
mod image_exports;
mod image_tasks;
#[cfg(test)]
mod import_tests;
mod palette;
mod project_save;
#[cfg(feature = "project-build")]
mod project_tasks;
mod recent_files;
mod worker;

use image_tasks::ImageCompletion;
use worker::Worker;

const INK: Color32 = Color32::from_rgb(26, 42, 48);
const WORKSPACE: Color32 = Color32::from_rgb(250, 252, 252);
const FOCUS: Color32 = Color32::from_rgb(0, 109, 107);
const ERROR: Color32 = Color32::from_rgb(165, 45, 45);
const HISTORY_KEY: Key = Key::Z;
const FILE_OPEN_KEY: Key = Key::O;

pub(crate) struct App {
	opened: Option<Opened>,
	worker: Option<Worker>,
	confirm: Option<Pending>,
	save_copy: bool,
	copy_name: String,
	closing: bool,
	error: Option<String>,
	palette: Option<palette::Palette>,
	comparison: image_compare::ComparePanel,
	recent: recent_files::RecentFiles,
	#[cfg(feature = "project-build")]
	build: project_tasks::BuildPanel,
}

enum Pending {
	Open,
	OpenRecent(recent_files::Entry),
	Close,
	#[cfg(feature = "project-build")]
	OpenBuiltImage(PathBuf),
}

enum Completion {
	Opened(Opened),
	RecentsLoaded {
		result: Result<recent_files::Loaded, recent_files::Error>,
		initial_path: Option<PathBuf>,
	},
	RecentsUpdated(Result<recent_files::Preferences, recent_files::Error>),
	Saved {
		path: PathBuf,
		revision: RevisionId,
		source: String,
		file: FileRevision,
	},
	Added {
		revision: RevisionId,
		edits: Result<Vec<ProjectEdit>, import::Error>,
	},
	Image(ImageCompletion),
	Compared(Result<image_compare::Completed, crate::image::compare::Error>),
	#[cfg(feature = "project-build")]
	Build(project_tasks::BuildCompletion),
	Cancelled,
	Failed(document::Error),
	SaveFailed(ProjectSaveError),
}

impl Completion {
	fn publication_outcome(&self) -> bool {
		match self {
			Self::Image(ImageCompletion::Exported(_)) => true,
			#[cfg(feature = "project-build")]
			Self::Build(_) => true,
			_ => false,
		}
	}
}

impl App {
	pub(crate) fn new(ctx: &Context, initial_path: Option<PathBuf>) -> Self {
		let mut visuals = egui::Visuals::light();
		visuals.panel_fill = WORKSPACE;
		visuals.override_text_color = Some(INK);
		visuals.selection.stroke.color = FOCUS;
		visuals.selection.bg_fill = Color32::from_rgb(198, 232, 225);
		ctx.set_visuals(visuals);
		let mut app = Self {
			opened: None,
			worker: None,
			confirm: None,
			save_copy: false,
			copy_name: String::new(),
			closing: false,
			error: None,
			palette: None,
			comparison: image_compare::ComparePanel::default(),
			recent: recent_files::RecentFiles::default(),
			#[cfg(feature = "project-build")]
			build: project_tasks::BuildPanel::default(),
		};
		if let Some(path) = initial_path {
			app.start(ctx, move || open_outcome(path));
		}
		app
	}

	fn start(&mut self, ctx: &Context, work: impl FnOnce() -> Completion + Send + 'static) {
		self.start_worker(ctx, None, work);
	}

	fn start_worker(
		&mut self,
		ctx: &Context,
		cancellation: Option<Cancellation>,
		work: impl FnOnce() -> Completion + Send + 'static,
	) {
		if self.worker.is_some() {
			return;
		}
		self.error = None;
		match Worker::start(ctx, cancellation, work) {
			Ok(worker) => self.worker = Some(worker),
			Err(error) => self.error = Some(format!("Could not start the document task. {error}")),
		}
	}

	fn poll(&mut self, ctx: &Context) {
		if let Some(worker) = &self.worker {
			match worker.receiver.try_recv() {
				Ok(result) => {
					let cancelled = worker
						.cancellation
						.as_ref()
						.is_some_and(Cancellation::is_cancelled);
					self.worker = None;
					// Publication may have completed before the cancellation reached the worker.
					if !cancelled || result.publication_outcome() {
						self.finish(ctx, result);
					}
				}
				Err(TryRecvError::Empty) => {}
				Err(TryRecvError::Disconnected) => {
					self.worker = None;
					#[cfg(feature = "project-build")]
					self.build.clear_progress();
					self.error = Some("The document task stopped before completing.".into());
				}
			}
		}
	}

	fn finish(&mut self, ctx: &Context, completion: Completion) {
		match completion {
			Completion::Opened(opened) => {
				self.remember_open(ctx, &opened);
				self.opened = Some(opened);
				self.comparison = image_compare::ComparePanel::default();
				#[cfg(feature = "project-build")]
				{
					self.build = project_tasks::BuildPanel::default();
				}
			}
			Completion::RecentsLoaded {
				result,
				initial_path,
			} => {
				self.finish_recent_load(ctx, result, initial_path);
			}
			Completion::RecentsUpdated(result) => self.finish_recent_update(result),
			Completion::Cancelled => {}
			Completion::Failed(error) => self.error = Some(error.to_string()),
			Completion::SaveFailed(error) => self.error = Some(error.to_string()),
			Completion::Image(result) => self.finish_image(ctx, result),
			Completion::Compared(result) => self.finish_comparison(result),
			#[cfg(feature = "project-build")]
			Completion::Build(result) => self.build.finish(result),
			Completion::Saved {
				path,
				revision,
				source,
				file,
			} => {
				if let Some(Opened {
					document: Document::Project(project),
					path: current,
				}) = &mut self.opened
				{
					project.mark_saved(revision, source, Some(file));
					*current = path;
				}
			}
			Completion::Added { revision, edits } => {
				if let Some(Opened {
					document: Document::Project(project),
					..
				}) = &mut self.opened
					&& project.document.revision() == revision
				{
					match edits {
						Ok(edits) => project.apply_edits(&edits),
						Err(error) => self.error = Some(error.to_string()),
					}
				}
			}
		}
	}

	fn choose(&mut self, ctx: &Context) {
		self.start(ctx, || {
			let selection = rfd::FileDialog::new()
				.add_filter(
					"Projects and disk images",
					&[PROJECT_EXTENSION, IMAGE_EXTENSION],
				)
				.pick_file();
			selection.map_or(Completion::Cancelled, open_outcome)
		});
	}

	fn add_files(
		&mut self,
		ctx: &Context,
		pick: impl FnOnce(&Path) -> Option<Vec<PathBuf>> + Send + 'static,
	) {
		if let Some(Opened {
			path,
			document: Document::Project(project),
		}) = &self.opened
			&& let Some(directory) = path.parent()
		{
			let directory = directory.to_path_buf();
			let folder = project.folder().clone();
			let revision = project.document.revision();
			self.start(ctx, move || {
				let Some(paths) = pick(&directory).filter(|paths| !paths.is_empty()) else {
					return Completion::Cancelled;
				};
				Completion::Added {
					revision,
					edits: import::prepare(&directory, &folder, paths),
				}
			});
		}
	}

	fn ask_open(&mut self, ctx: &Context) {
		if self.is_dirty() {
			self.confirm = Some(Pending::Open);
		} else {
			self.choose(ctx);
		}
	}

	fn is_dirty(&self) -> bool {
		matches!(&self.opened, Some(Opened { document: Document::Project(project), .. }) if project.is_dirty())
	}

	fn toolbar(&mut self, ui: &mut Ui) {
		Panel::top("document_toolbar").show(ui, |ui| {
			ui.horizontal_wrapped(|ui| {
				ui.add_enabled_ui(self.worker.is_none(), |ui| {
					if ui.button("Open…").clicked() {
						self.ask_open(ui.ctx());
					}
					self.recent_menu(ui);
					self.palette_button(ui);
					self.compare_action(ui);
					if matches!(
						&self.opened,
						Some(Opened {
							document: Document::Project(_),
							..
						})
					) {
						if ui.button("Add files…").clicked() {
							self.add_files(ui.ctx(), import::pick_files);
						}
						self.save_actions(ui);
						#[cfg(feature = "project-build")]
						self.build_actions(ui);
						if let Some(Opened {
							document: Document::Project(project),
							..
						}) = &mut self.opened
						{
							if ui.button("Undo").clicked() {
								project.undo();
							}
							if ui.button("Redo").clicked() {
								project.redo();
							}
						}
					}
				});
				ui.separator();
				if let Some(opened) = &self.opened {
					ui.label(RichText::new(display_name(&opened.path)).strong())
						.on_hover_text(opened.path.display().to_string());
					if self.is_dirty() {
						ui.weak("Unsaved changes");
					}
				}
				if let Some(worker) = &self.worker {
					ui.spinner();
					match &worker.cancellation {
						Some(cancellation) if cancellation.is_cancelled() => {
							ui.label("Cancelling…");
						}
						Some(cancellation) => {
							ui.label("Working…");
							if ui.button("Cancel").clicked() {
								cancellation.cancel();
							}
						}
						None => {
							ui.label("Working…");
						}
					}
				}
			});
			if let Some(error) = &self.error {
				ui.colored_label(ERROR, error);
			}
			if let Some(error) = &self.recent.error {
				ui.colored_label(ERROR, error);
			}
			#[cfg(feature = "project-build")]
			self.show_build(ui);
		});
	}

	fn modal(&mut self, ctx: &Context) {
		if self.confirm.is_some() {
			let modal = egui::Modal::new(Id::unique("discard_edits")).show(ctx, |ui| {
				ui.heading("Discard unsaved changes?");
				ui.horizontal(|ui| {
					let keep = ui.button("Keep editing").clicked();
					let discard = ui.button("Discard").clicked();
					(keep, discard)
				})
				.inner
			});
			let (keep, discard) = modal.inner;
			if discard {
				match self.confirm.take() {
					Some(Pending::Open) => self.choose(ctx),
					Some(Pending::OpenRecent(entry)) => self.open_recent(ctx, entry),
					#[cfg(feature = "project-build")]
					Some(Pending::OpenBuiltImage(path)) => {
						self.start(ctx, move || project_tasks::open_built_outcome(path));
					}
					Some(Pending::Close) => {
						self.closing = true;
						ctx.send_viewport_cmd(ViewportCommand::Close);
					}
					None => {}
				}
			} else if keep || modal.should_close() {
				self.confirm = None;
			}
		}
		self.save_copy_modal(ctx);
	}

	fn shortcuts(&mut self, ctx: &Context) {
		if self.worker.is_some() || self.confirm.is_some() || self.save_copy {
			return;
		}
		if self.palette_shortcut(ctx) {
			return;
		}
		if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, FILE_OPEN_KEY)) {
			self.ask_open(ctx);
			return;
		}
		if self.save_shortcut(ctx) {
			return;
		}
		if ctx.text_edit_focused() {
			return;
		}
		if let Some(Opened {
			document: Document::Project(project),
			..
		}) = &mut self.opened
		{
			let redo = Modifiers::COMMAND | Modifiers::SHIFT;
			if ctx.input_mut(|input| input.consume_key(redo, HISTORY_KEY)) {
				project.redo();
			} else if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, HISTORY_KEY)) {
				project.undo();
			}
		}
	}
}

impl eframe::App for App {
	fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
		self.poll(ctx);
	}

	fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
		let ctx = ui.ctx().clone();
		if !self.closing
			&& ctx.input(|input| input.viewport().close_requested())
			&& !self.defer_close_for_worker(&ctx)
			&& self.is_dirty()
		{
			ctx.send_viewport_cmd(ViewportCommand::CancelClose);
			self.confirm = Some(Pending::Close);
			self.save_copy = false;
		}
		self.shortcuts(&ctx);
		self.toolbar(ui);
		let idle = self.worker.is_none();
		let image_action = match &mut self.opened {
			Some(Opened {
				document: Document::Project(project),
				..
			}) => {
				ui.add_enabled_ui(idle, |ui| project.ui(ui));
				None
			}
			Some(Opened {
				document: Document::Image(image),
				..
			}) => inspector::show(ui, image, idle),
			None => {
				egui::CentralPanel::default().show(ui, |ui| {
					ui.vertical_centered(|ui| {
						ui.add_space(ui.available_height() / 3.0);
						ui.heading("Open a project or disk image");
						if ui
							.add_enabled(idle, egui::Button::new("Choose file…"))
							.clicked()
						{
							self.choose(&ctx);
						}
					});
				});
				None
			}
		};
		if let Some(action) = image_action {
			self.image_action(&ctx, action);
		}
		self.show_comparison(&ctx);
		self.show_palette(&ctx);
		self.modal(&ctx);
	}
}

fn open_outcome(path: PathBuf) -> Completion {
	match document::open(path) {
		Ok(opened) => Completion::Opened(opened),
		Err(error) => Completion::Failed(error),
	}
}

fn display_name(path: &Path) -> String {
	path.file_name().map_or_else(
		|| path.display().to_string(),
		|name| name.to_string_lossy().into_owned(),
	)
}

#[cfg(test)]
mod document_tests;
