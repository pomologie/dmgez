use std::{
	path::{Path, PathBuf},
	sync::mpsc::{self, Receiver, TryRecvError},
	thread,
};

use dmgez_config::project_edit::{ProjectEdit, RevisionId};
use eframe::egui::{
	self, Color32, Context, Id, Key, Modifiers, Panel, RichText, TextEdit, Ui, ViewportCommand,
};

use crate::document::{self, Document, IMAGE_EXTENSION, Opened, PROJECT_EXTENSION};
use crate::import;

#[cfg(test)]
mod import_tests;

const WORKER_NAME: &str = "dmgez document";
const PROJECT_COPY: &str = "project-copy.toml";
const INK: Color32 = Color32::from_rgb(26, 42, 48);
const WORKSPACE: Color32 = Color32::from_rgb(250, 252, 252);
const FOCUS: Color32 = Color32::from_rgb(0, 109, 107);
const ERROR: Color32 = Color32::from_rgb(165, 45, 45);
const HISTORY_KEY: Key = Key::Z;
const FILE_OPEN_KEY: Key = Key::O;

pub(crate) struct App {
	opened: Option<Opened>,
	worker: Option<Receiver<Completion>>,
	confirm: Option<Pending>,
	save_copy: bool,
	copy_name: String,
	closing: bool,
	error: Option<String>,
}

enum Pending {
	Open,
	Close,
}

enum Completion {
	Opened(Opened),
	Saved {
		path: PathBuf,
		revision: RevisionId,
	},
	Added {
		revision: RevisionId,
		edits: Result<Vec<ProjectEdit>, import::Error>,
	},
	Cancelled,
	Failed(document::Error),
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
		};
		if let Some(path) = initial_path {
			app.start(ctx, move || open_outcome(path));
		}
		app
	}

	fn start(&mut self, ctx: &Context, work: impl FnOnce() -> Completion + Send + 'static) {
		if self.worker.is_some() {
			return;
		}
		self.error = None;
		let (send, receive) = mpsc::sync_channel(1);
		let repaint = ctx.clone();
		match thread::Builder::new()
			.name(WORKER_NAME.into())
			.spawn(move || {
				let _ = send.send(work());
				repaint.request_repaint();
			}) {
			Ok(_) => self.worker = Some(receive),
			Err(error) => self.error = Some(format!("Could not start the document task. {error}")),
		}
	}

	fn poll(&mut self) {
		if let Some(worker) = &self.worker {
			match worker.try_recv() {
				Ok(result) => {
					self.worker = None;
					self.finish(result);
				}
				Err(TryRecvError::Empty) => {}
				Err(TryRecvError::Disconnected) => {
					self.worker = None;
					self.error = Some("The document task stopped before completing.".into());
				}
			}
		}
	}

	fn finish(&mut self, completion: Completion) {
		match completion {
			Completion::Opened(opened) => self.opened = Some(opened),
			Completion::Cancelled => {}
			Completion::Failed(error) => self.error = Some(error.to_string()),
			Completion::Saved { path, revision } => {
				if let Some(Opened {
					document: Document::Project(project),
					path: current,
				}) = &mut self.opened
				{
					project.mark_saved(revision);
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

	fn open_save_dialog(&mut self) {
		self.copy_name.clear();
		self.copy_name.push_str(PROJECT_COPY);
		self.save_copy = true;
	}

	fn write_copy(&mut self, ctx: &Context) {
		if let Some(Opened {
			path,
			document: Document::Project(project),
		}) = &self.opened
		{
			let path = path.clone();
			let filename = self.copy_name.clone();
			let revision = project.document.revision();
			let source = project.document.source().to_owned();
			self.start(ctx, move || {
				match document::save_copy(&path, &filename, &source) {
					Ok(path) => Completion::Saved { path, revision },
					Err(error) => Completion::Failed(error),
				}
			});
		}
	}

	fn toolbar(&mut self, ui: &mut Ui) {
		Panel::top("document_toolbar").show(ui, |ui| {
			ui.horizontal_wrapped(|ui| {
				ui.add_enabled_ui(self.worker.is_none(), |ui| {
					if ui.button("Open…").clicked() {
						self.ask_open(ui.ctx());
					}
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
						if ui.button("Save copy…").clicked() {
							self.open_save_dialog();
						}
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
				if self.worker.is_some() {
					ui.spinner();
					ui.label("Working…");
				}
			});
			if let Some(error) = &self.error {
				ui.colored_label(ERROR, error);
			}
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
		if self.save_copy {
			let modal = egui::Modal::new(Id::unique("save_project_copy")).show(ctx, |ui| {
				ui.heading("Save a project copy");
				ui.label("The copy is saved beside the source project.");
				ui.add(TextEdit::singleline(&mut self.copy_name).hint_text("Filename"));
				ui.horizontal(|ui| {
					let cancel = ui.button("Cancel").clicked();
					let save = ui.button("Save copy").clicked();
					(cancel, save)
				})
				.inner
			});
			let (cancel, save) = modal.inner;
			if save {
				self.save_copy = false;
				self.write_copy(ctx);
			} else if cancel || modal.should_close() {
				self.save_copy = false;
			}
		}
	}

	fn shortcuts(&mut self, ctx: &Context) {
		if self.worker.is_some() || self.confirm.is_some() || self.save_copy {
			return;
		}
		if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, FILE_OPEN_KEY)) {
			self.ask_open(ctx);
			return;
		}
		if ctx.egui_wants_keyboard_input() {
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
	fn logic(&mut self, _ctx: &Context, _frame: &mut eframe::Frame) {
		self.poll();
	}

	fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
		let ctx = ui.ctx().clone();
		if !self.closing && ctx.input(|input| input.viewport().close_requested()) && self.is_dirty()
		{
			ctx.send_viewport_cmd(ViewportCommand::CancelClose);
			self.confirm = Some(Pending::Close);
			self.save_copy = false;
		}
		self.shortcuts(&ctx);
		self.toolbar(ui);
		ui.add_enabled_ui(self.worker.is_none(), |ui| match &mut self.opened {
			Some(Opened {
				document: Document::Project(project),
				..
			}) => project.ui(ui),
			Some(Opened {
				document: Document::Image(report),
				..
			}) => crate::inspector::show(ui, report),
			None => {
				egui::CentralPanel::default().show(ui, |ui| {
					ui.vertical_centered(|ui| {
						ui.add_space(ui.available_height() / 3.0);
						ui.heading("Open a project or disk image");
						if ui.button("Choose file…").clicked() {
							self.choose(&ctx);
						}
					});
				});
			}
		});
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
mod tests {
	use super::*;

	#[test]
	fn asynchronous_failure_and_cancel_leave_previous_document_available() {
		let mut app = App::new(&Context::default(), None);
		let scratch = tempfile::tempdir().unwrap();
		let file = scratch.path().join("retained.toml");
		std::fs::write(
			&file,
			"schema_version=1\n[image]\nname='Original'\noutput='build.dmg'",
		)
		.unwrap();
		app.finish(open_outcome(file.clone()));
		app.finish(Completion::Cancelled);
		app.finish(open_outcome(file.with_extension("exe")));
		assert_eq!(
			app.opened.as_ref().unwrap().path,
			file.canonicalize().unwrap()
		);
		assert!(app.error.is_some());
	}
}
