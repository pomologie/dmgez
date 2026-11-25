use dmgez_runtime::project_save::{replace_supported, save};
use eframe::egui::{self, Button, Context, Id, Key, Modifiers, TextEdit, Ui};

use crate::document::{self, Document, Opened};

use super::{App, Completion};

const PROJECT_COPY: &str = "project-copy.toml";
const FILE_SAVE_KEY: Key = Key::S;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

impl App {
	pub(super) fn save_actions(&mut self, ui: &mut Ui) {
		let supported = replace_supported();
		let save_button = ui
			.add_enabled(supported && self.is_dirty(), Button::new("Save"))
			.on_disabled_hover_text(if supported {
				"The project has no unsaved changes."
			} else {
				"Use Save copy on this platform."
			});
		if save_button.clicked() {
			self.save_project(ui.ctx());
		}
		if ui.button("Save copy…").clicked() {
			self.ask_save_copy();
		}
	}

	pub(super) fn ask_save_copy(&mut self) {
		self.copy_name.clear();
		self.copy_name.push_str(PROJECT_COPY);
		self.save_copy = true;
	}

	pub(super) fn save_shortcut(&mut self, ctx: &Context) -> bool {
		if !ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, FILE_SAVE_KEY)) {
			return false;
		}
		self.save_project(ctx);
		true
	}

	pub(super) fn save_project(&mut self, ctx: &Context) {
		if let Some(Opened {
			path,
			document: Document::Project(project),
		}) = &self.opened
			&& project.is_dirty()
			&& let Some((base, expected)) = project.saved_file()
		{
			let path = path.clone();
			let revision = project.document.revision();
			let source = project.document.source().to_owned();
			let expected = expected.to_owned();
			self.start(ctx, move || match save(&path, base, &expected, &source) {
				Ok(file) => Completion::Saved {
					path,
					revision,
					source,
					file,
				},
				Err(error) => Completion::SaveFailed(error),
			});
		}
	}

	pub(super) fn save_copy_modal(&mut self, ctx: &Context) {
		if !self.save_copy {
			return;
		}
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
					Ok((path, file)) => Completion::Saved {
						path,
						revision,
						source,
						file,
					},
					Err(error) => Completion::Failed(error),
				}
			});
		}
	}
}
