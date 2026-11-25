use dmgez_config::{
	VirtualPath,
	project_edit::{ContentDraft, ProjectEdit, ProjectEditError},
};
use eframe::egui::{TextEdit, Ui};
use thiserror::Error;

use super::{Action, PATH_SEPARATOR, ProjectTab};

#[cfg(test)]
mod tests;

const APPLICATIONS_NAME: &str = "Applications";
const APPLICATIONS_TARGET: &str = "/Applications";
const DEFINE_START: &str = "${";

#[derive(Clone, Copy)]
enum EntryKind {
	Directory,
	Symlink,
}

#[derive(Clone, Copy)]
enum MenuAction {
	Open(EntryKind),
	Applications,
}

enum FormAction {
	Create,
	Cancel,
}

pub(super) struct EntryDraft {
	kind: EntryKind,
	name: String,
	target: String,
	focus_name: bool,
	error: Option<String>,
}

#[derive(Debug, Error)]
enum CreateError {
	#[error("Choose a folder in the project.")]
	Folder,
	#[error("Enter a single valid name in the current folder.")]
	Name,
	#[error(
		"An entry path or link target cannot contain the project variable marker {DEFINE_START:?}."
	)]
	VariableSyntax,
	#[error("An entry named {0:?} already exists in this folder.")]
	Conflict(String),
	#[error(transparent)]
	Edit(#[from] ProjectEditError),
}

const MENU: [(&str, MenuAction); 3] = [
	("New folder…", MenuAction::Open(EntryKind::Directory)),
	("New symbolic link…", MenuAction::Open(EntryKind::Symlink)),
	("Add Applications shortcut", MenuAction::Applications),
];

impl EntryDraft {
	fn new(kind: EntryKind) -> Self {
		Self {
			kind,
			name: String::new(),
			target: String::new(),
			focus_name: true,
			error: None,
		}
	}

	fn target(&self) -> Option<&str> {
		match self.kind {
			EntryKind::Directory => None,
			EntryKind::Symlink => Some(&self.target),
		}
	}

	fn show(&mut self, ui: &mut Ui) -> Option<FormAction> {
		let name_label = ui.label(match self.kind {
			EntryKind::Directory => "Folder name",
			EntryKind::Symlink => "Symbolic link name",
		});
		let name = ui
			.add(TextEdit::singleline(&mut self.name))
			.labelled_by(name_label.id);
		if self.focus_name && name.enabled() {
			name.request_focus();
			self.focus_name = false;
		}
		let mut changed = name.changed();
		if matches!(self.kind, EntryKind::Symlink) {
			let target_label = ui.label("Link target");
			changed |= ui
				.add(TextEdit::singleline(&mut self.target))
				.labelled_by(target_label.id)
				.changed();
		}
		if changed {
			self.error = None;
		}
		if let Some(error) = &self.error {
			ui.label(error);
		}
		ui.horizontal(|ui| {
			if ui.button("Create").clicked() {
				Some(FormAction::Create)
			} else if ui.button("Cancel").clicked() {
				Some(FormAction::Cancel)
			} else {
				None
			}
		})
		.inner
	}
}

fn show_menu(ui: &mut Ui, enabled: bool) -> Option<MenuAction> {
	ui.add_enabled_ui(enabled, |ui| {
		ui.menu_button("Add entry", |ui| {
			for (label, action) in MENU {
				if ui.button(label).clicked() {
					ui.close();
					return Some(action);
				}
			}
			None
		})
		.inner
		.flatten()
	})
	.inner
}

impl ProjectTab {
	pub(super) fn entry_creator(&mut self, ui: &mut Ui) {
		if let Some(mut draft) = self.entry_draft.take() {
			match draft.show(ui) {
				Some(FormAction::Create) => {
					if let Err(error) = self.create_entry(&draft.name, draft.target()) {
						draft.error = Some(error.to_string());
						self.entry_draft = Some(draft);
					}
				}
				Some(FormAction::Cancel) => {}
				None => self.entry_draft = Some(draft),
			}
			return;
		}
		let can_add = self
			.tree
			.as_ref()
			.is_ok_and(|tree| tree.contains_folder(&self.folder));
		match show_menu(ui, can_add) {
			Some(MenuAction::Open(kind)) => self.entry_draft = Some(EntryDraft::new(kind)),
			Some(MenuAction::Applications) => {
				if let Err(error) = self.create_entry(APPLICATIONS_NAME, Some(APPLICATIONS_TARGET))
				{
					self.error = Some(error.to_string());
				}
			}
			None => {}
		}
	}

	fn create_entry(&mut self, name: &str, target: Option<&str>) -> Result<(), CreateError> {
		let tree = self.tree.as_ref().map_err(|_| CreateError::Folder)?;
		if !tree.contains_folder(&self.folder) {
			return Err(CreateError::Folder);
		}
		let path = child_path(&self.folder, name)?;
		if path.as_str().contains(DEFINE_START)
			|| target.is_some_and(|target| target.contains(DEFINE_START))
		{
			return Err(CreateError::VariableSyntax);
		}
		if tree
			.children(&self.folder)
			.binary_search_by(|entry| entry.path().cmp(&path))
			.is_ok()
		{
			return Err(CreateError::Conflict(name.to_owned()));
		}
		let draft = match target {
			Some(target) => ContentDraft::Symlink {
				destination: path.clone(),
				target: target.to_owned(),
			},
			None => ContentDraft::Directory {
				destination: path.clone(),
			},
		};
		if self.document.apply(&[ProjectEdit::Add(draft)])? {
			self.refresh_document();
			self.handle_action(Action::Select {
				path,
				toggle: false,
			});
		}
		Ok(())
	}
}

fn child_path(folder: &VirtualPath, name: &str) -> Result<VirtualPath, CreateError> {
	if name.is_empty() || name.contains(PATH_SEPARATOR) || name.chars().any(char::is_control) {
		return Err(CreateError::Name);
	}
	let prefix = if folder.is_root() {
		""
	} else {
		folder.as_str()
	};
	format!("{prefix}{PATH_SEPARATOR}{name}")
		.parse()
		.map_err(|_| CreateError::Name)
}
