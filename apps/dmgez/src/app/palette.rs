use eframe::egui::{self, Context, Id, Key, KeyboardShortcut, Modifiers, TextEdit, Ui};

use crate::{image::exports::Kind as Export, import, inspector};

use super::{App, Document, Opened};

const PALETTE_ID: &str = "command_palette";
const SEARCH_ID: &str = "command_palette_search";
const SHORTCUT: KeyboardShortcut =
	KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::P);
const SEARCH_LIMIT: usize = 80;
const WIDTH: f32 = 360.0;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Palette {
	query: String,
	focus_requested: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
	Open,
	Add,
	Save,
	SaveCopy,
	Undo,
	Redo,
	#[cfg(feature = "project-build")]
	Plan,
	#[cfg(feature = "project-build")]
	Build,
	Compare,
	Verify,
	ExportInspection,
	ExportVerification,
}

const PROJECT_COMMANDS: &[Command] = &[
	Command::Add,
	Command::SaveCopy,
	Command::Undo,
	Command::Redo,
	#[cfg(feature = "project-build")]
	Command::Plan,
	#[cfg(feature = "project-build")]
	Command::Build,
];

impl Command {
	fn label(self) -> &'static str {
		match self {
			Self::Open => "Open file…",
			Self::Add => "Add project files…",
			Self::Save => "Save project",
			Self::SaveCopy => "Save project copy…",
			Self::Undo => "Undo project edit",
			Self::Redo => "Redo project edit",
			#[cfg(feature = "project-build")]
			Self::Plan => "Plan image build",
			#[cfg(feature = "project-build")]
			Self::Build => "Build image…",
			Self::Compare => "Compare disk images…",
			Self::Verify => "Verify container",
			Self::ExportInspection => "Export inspection JSON…",
			Self::ExportVerification => "Export verification JSON…",
		}
	}
}

impl App {
	pub(super) fn palette_button(&mut self, ui: &mut Ui) {
		let shortcut = ui.ctx().format_shortcut(&SHORTCUT);
		if ui.button("Commands…").on_hover_text(shortcut).clicked() {
			self.open_palette();
		}
	}

	pub(super) fn palette_shortcut(&mut self, ctx: &Context) -> bool {
		if ctx.input_mut(|input| input.consume_shortcut(&SHORTCUT)) {
			self.open_palette();
		}
		self.palette.is_some()
	}

	fn open_palette(&mut self) {
		self.palette = Some(Palette::default());
	}

	pub(super) fn show_palette(&mut self, ctx: &Context) {
		if self.palette.is_none() {
			return;
		}
		if self.worker.is_some() || self.confirm.is_some() || self.save_copy {
			self.palette = None;
			return;
		}
		let commands = self.available_commands();
		let Some(state) = &mut self.palette else {
			return;
		};
		let palette = egui::Modal::new(Id::unique(PALETTE_ID)).show(ctx, |ui| {
			ui.set_width(WIDTH);
			ui.heading("Commands");
			let search_id = Id::unique(SEARCH_ID);
			let enter = ui.memory(|memory| memory.has_focus(search_id))
				&& ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
			let label = ui.label("Search commands");
			let search = ui
				.add(
					TextEdit::singleline(&mut state.query)
						.id(search_id)
						.char_limit(SEARCH_LIMIT)
						.desired_width(WIDTH),
				)
				.labelled_by(label.id);
			if !state.focus_requested && !ui.is_sizing_pass() {
				search.request_focus();
				state.focus_requested = true;
			}
			let terms = state.query.to_ascii_lowercase();
			let mut matches = commands.iter().copied().filter(|command| {
				let label = command.label().to_ascii_lowercase();
				terms.split_whitespace().all(|term| label.contains(term))
			});
			let Some(first) = matches.next() else {
				ui.label("No matching commands");
				return None;
			};
			let mut selected = enter.then_some(first);
			for command in std::iter::once(first).chain(matches) {
				if ui.button(command.label()).clicked() {
					selected = Some(command);
				}
			}
			selected
		});
		if let Some(command) = palette.inner {
			self.palette = None;
			self.dispatch_command(ctx, command);
		} else if palette.should_close() {
			self.palette = None;
		}
	}

	fn available_commands(&self) -> Vec<Command> {
		let mut commands = vec![Command::Open];
		match self.opened.as_ref().map(|opened| &opened.document) {
			Some(Document::Project(project)) => {
				commands.extend_from_slice(PROJECT_COMMANDS);
				if dmgez_runtime::project_save::replace_supported() && project.is_dirty() {
					commands.push(Command::Save);
				}
			}
			Some(Document::Image(image)) => {
				commands.extend([Command::Compare, Command::Verify, Command::ExportInspection]);
				if image.verification.is_some() {
					commands.push(Command::ExportVerification);
				}
			}
			None => {}
		}
		commands
	}

	fn dispatch_command(&mut self, ctx: &Context, command: Command) {
		match command {
			Command::Open => self.ask_open(ctx),
			Command::Add => self.add_files(ctx, import::pick_files),
			Command::Save => self.save_project(ctx),
			Command::SaveCopy => self.ask_save_copy(),
			Command::Undo | Command::Redo => {
				if let Some(Opened {
					document: Document::Project(project),
					..
				}) = &mut self.opened
				{
					match command {
						Command::Undo => project.undo(),
						_ => project.redo(),
					}
				}
			}
			#[cfg(feature = "project-build")]
			Command::Plan => self.build_from_editor(ctx, dmgez_runtime::project_build::BuildMode::Plan),
			#[cfg(feature = "project-build")]
			Command::Build => self.build_from_editor(ctx, dmgez_runtime::project_build::BuildMode::Publish),
			Command::Compare => self.open_comparison(),
			Command::Verify => self.image_action(ctx, inspector::Action::Verify),
			Command::ExportInspection => self.start_image_export(ctx, Export::Inspection),
			Command::ExportVerification => self.start_image_export(ctx, Export::Verification),
		}
	}
}
