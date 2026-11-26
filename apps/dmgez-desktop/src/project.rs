use std::collections::BTreeSet;

use dmgez_config::{
	Project, VirtualPath,
	project_edit::{
		ContentChanges, FieldChange, ProjectDocument, ProjectEdit, ProjectEditError, RevisionId,
	},
};
use dmgez_preview::{
	DeclaredTree, DeclaredTreeError, Drag, FolderPoint, PositionSource, ProjectFolder,
};
use dmgez_runtime::project_save::FileRevision;
use eframe::egui::{self, Button, DragValue, Panel, ScrollArea, TextEdit, Ui};

use crate::layout_lint::LayoutLint;

pub(crate) mod arrangement;
mod create;
pub(crate) mod input;
#[cfg(test)]
mod tests;

use arrangement::Arrangement;

const IMAGE_ROOT: &str = "/";
const PATH_SEPARATOR: char = '/';
const MAX_CANVAS_ENTRIES: usize = 512;
const MIN_ZOOM_PERCENT: u32 = 25;
const MAX_ZOOM_PERCENT: u32 = 200;
const DEFAULT_ZOOM_PERCENT: u32 = 100;
const POSITION_SPEED: f64 = 0.25;
const LIST_WIDTH: f32 = 216.0;
const DETAILS_WIDTH: f32 = 226.0;

pub(crate) struct ProjectTab {
	pub document: ProjectDocument,
	saved: RevisionId,
	saved_source: String,
	saved_file: Option<FileRevision>,
	tree: Result<DeclaredTree, DeclaredTreeError>,
	folder: VirtualPath,
	selected: BTreeSet<VirtualPath>,
	zoom: u32,
	show_hidden: bool,
	snap: bool,
	layout_lint: LayoutLint,
	image_name: String,
	position_draft: Option<(VirtualPath, [f64; 2])>,
	entry_draft: Option<create::EntryDraft>,
	pub(crate) drag: Option<DragEdit>,
	error: Option<String>,
}

pub(crate) struct DragEdit {
	pub path: VirtualPath,
	pub drag: Drag,
	pub center: FolderPoint,
}

impl ProjectTab {
	pub(crate) fn new(document: ProjectDocument) -> Self {
		let project = document.project();
		Self {
			saved: document.revision(),
			saved_source: document.source().to_owned(),
			saved_file: None,
			tree: DeclaredTree::from_project(project),
			folder: IMAGE_ROOT.parse().expect("root is a valid virtual path"),
			selected: BTreeSet::new(),
			zoom: DEFAULT_ZOOM_PERCENT,
			show_hidden: false,
			snap: false,
			layout_lint: LayoutLint::default(),
			image_name: project.image.name.as_str().to_owned(),
			position_draft: None,
			entry_draft: None,
			drag: None,
			error: None,
			document,
		}
	}

	pub(crate) fn is_dirty(&self) -> bool {
		self.document.revision() != self.saved
	}

	pub(crate) fn with_file_revision(mut self, file: FileRevision) -> Self {
		self.saved_file = Some(file);
		self
	}

	pub(crate) fn saved_file(&self) -> Option<(FileRevision, &str)> {
		self.saved_file
			.map(|file| (file, self.saved_source.as_str()))
	}

	pub(crate) fn mark_saved(
		&mut self,
		revision: RevisionId,
		source: String,
		file: Option<FileRevision>,
	) {
		self.saved = revision;
		self.saved_source = source;
		self.saved_file = file;
	}

	pub(crate) fn folder(&self) -> &VirtualPath {
		&self.folder
	}

	pub(crate) fn apply_edits(&mut self, edits: &[ProjectEdit]) {
		self.update(|document| document.apply(edits));
	}

	fn apply(&mut self, edit: ProjectEdit) {
		self.apply_edits(&[edit]);
	}

	fn update(
		&mut self,
		edit: impl FnOnce(&mut ProjectDocument) -> Result<bool, ProjectEditError>,
	) {
		match edit(&mut self.document) {
			Ok(true) => self.refresh_document(),
			Ok(false) => {}
			Err(error) => self.error = Some(error.to_string()),
		}
	}

	fn refresh_document(&mut self) {
		let project = self.document.project();
		self.tree = DeclaredTree::from_project(project);
		self.image_name = project.image.name.as_str().to_owned();
		if let Ok(tree) = &self.tree {
			let mut folder = self.folder.clone();
			while !tree.contains_folder(&folder) {
				folder = parent(folder.as_str())
					.parse()
					.expect("the parent of a validated virtual path is valid");
			}
			if folder != self.folder {
				self.navigate(folder);
			}
		}
		self.selected.retain(|path| {
			self.tree.as_ref().is_ok_and(|tree| {
				tree.children(&self.folder)
					.binary_search_by(|entry| entry.path().cmp(path))
					.is_ok()
			})
		});
		self.position_draft = None;
		self.error = None;
	}

	pub(crate) fn undo(&mut self) {
		self.update(ProjectDocument::undo);
	}

	pub(crate) fn redo(&mut self) {
		self.update(ProjectDocument::redo);
	}

	fn navigate(&mut self, folder: VirtualPath) {
		self.folder = folder;
		self.selected.clear();
		self.position_draft = None;
		self.entry_draft = None;
		self.drag = None;
	}

	pub(crate) fn ui(&mut self, ui: &mut Ui) {
		if let Err(error) = &self.tree {
			ui.label(error.to_string());
			return;
		}
		let mut action = None;
		Panel::left("project_entries")
			.default_size(LIST_WIDTH)
			.show(ui, |ui| {
				ui.heading("Contents");
				action = self.entries(ui);
			});
		if let Some(action) = action {
			self.handle_action(action);
		}
		Panel::right("project_properties")
			.default_size(DETAILS_WIDTH)
			.show(ui, |ui| {
				self.details(ui);
			});
		egui::CentralPanel::default().show(ui, |ui| {
			self.design(ui);
		});
	}

	fn entries(&mut self, ui: &mut Ui) -> Option<Action> {
		let mut action = None;
		ui.horizontal(|ui| {
			let go_up = ui
				.add_enabled(!self.folder.is_root(), Button::new("Up"))
				.clicked();
			if go_up && let Ok(folder) = parent(self.folder.as_str()).parse() {
				action = Some(Action::Navigate(folder));
			}
			ui.label(self.folder.as_str());
		});
		self.entry_creator(ui);
		let tree = self.tree.as_ref().ok()?;
		let entries = tree.children(&self.folder);
		let row_height = ui.spacing().interact_size.y;
		ScrollArea::vertical().show_rows(ui, row_height, entries.len(), |ui, rows| {
			for row in rows {
				let entry = &entries[row];
				let name = name(entry.path());
				let selected = self.selected.contains(entry.path());
				ui.horizontal(|ui| {
					let response = ui.selectable_label(selected, name).on_hover_text(
						"Shift-click or Ctrl/Cmd-click to toggle. Arrow keys nudge selected icons.",
					);
					if response.clicked() {
						action = Some(input::select(ui, entry.path()));
					} else if action.is_none()
						&& selected && self.drag.is_none()
						&& let Some(keyboard) = input::nudge(ui, &response)
					{
						action = Some(keyboard);
					}
					if entry.is_directory() && ui.small_button("Open").clicked() {
						action = Some(Action::Navigate(entry.path().clone()));
					}
				});
			}
		});
		action
	}

	fn details(&mut self, ui: &mut Ui) {
		ui.heading("Properties");
		ui.label("Volume label");
		ui.add(TextEdit::singleline(&mut self.image_name).desired_width(f32::INFINITY));
		if ui.button("Apply label").clicked() {
			let name = self.image_name.clone();
			self.apply(ProjectEdit::SetImageName(name));
		}
		ui.separator();
		if self.selected.len() > 1 {
			ui.label(format!("{} entries selected", self.selected.len()));
			return;
		}
		let Some(path) = self.selected.first().cloned() else {
			return;
		};
		ui.strong(name(&path));
		ui.small(path.as_str());
		let editable = editable(self.document.project(), &path);
		let position = self.layout().ok().and_then(|layout| {
			layout
				.items()
				.iter()
				.find(|item| *item.destination == path)
				.map(|item| (item.center, item.position_source))
		});
		if let Some((point, source)) = position {
			let position_enabled = editable && source != PositionSource::FinderFolder;
			let current = [point.x(), point.y()];
			if self
				.position_draft
				.as_ref()
				.is_none_or(|(selected, _)| *selected != path)
			{
				self.position_draft = Some((path.clone(), current));
			}
			if let Some((_, draft @ [_, _])) = &mut self.position_draft {
				let [x, y] = &mut *draft;
				ui.add_enabled(
					position_enabled,
					DragValue::new(x).speed(POSITION_SPEED).prefix("X "),
				);
				ui.add_enabled(
					position_enabled,
					DragValue::new(y).speed(POSITION_SPEED).prefix("Y "),
				);
				let position = *draft;
				if ui
					.add_enabled(
						position_enabled && position != current,
						Button::new("Apply position"),
					)
					.clicked()
				{
					self.set_position(path.clone(), position);
				}
			}
		}
		if let Some(content) = self
			.document
			.project()
			.contents
			.iter()
			.find(|item| item.destination == path)
		{
			let mut hidden = content.hidden.unwrap_or_default();
			if ui.checkbox(&mut hidden, "Hidden in Finder").changed() {
				let mut changes = ContentChanges::default();
				changes.hidden = FieldChange::Set(hidden);
				self.apply(ProjectEdit::Modify {
					target: path,
					changes,
				});
			}
		}
	}

	fn design(&mut self, ui: &mut Ui) {
		if let Some(error) = &self.error {
			ui.label(error);
		}
		let project = self.document.project();
		let (action, active_drag) = match folder_layout(project, &self.tree, &self.folder) {
			Ok(layout) => {
				let (movable, arranged) = ui
					.horizontal_wrapped(|ui| {
						ui.heading("Layout");
						ui.add(
							egui::Slider::new(&mut self.zoom, MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT)
								.suffix("%"),
						);
						ui.checkbox(&mut self.show_hidden, "Show hidden");
						ui.checkbox(&mut self.snap, "Snap to grid");
						self.layout_lint.toggle(ui);
						let movable = arrangement::movable_selection(
							&layout,
							project,
							&self.selected,
							self.show_hidden,
						)
						.filter(|_| self.drag.is_none())
						.map_or(0, |items| items.len());
						(
							movable,
							arrangement::toolbar(ui, movable).map(Action::Arrange),
						)
					})
					.inner;
				let declared_folder = self
					.tree
					.as_ref()
					.is_ok_and(|tree| tree.contains_folder(&self.folder));
				let lint_warnings = self.layout_lint.show(
					ui,
					self.document.revision(),
					&self.folder,
					declared_folder.then_some(&layout),
				);
				let scale = f64::from(self.zoom) / f64::from(DEFAULT_ZOOM_PERCENT);
				let view = crate::canvas::View {
					selected: &self.selected,
					nudge_enabled: movable != 0,
					show_hidden: self.show_hidden,
					snap: self.snap,
					scale,
					drag: self.drag.as_ref(),
					lint_warnings,
				};
				let (action, drag) =
					crate::canvas::show(ui, &layout, view, |path| editable(project, path));
				(arranged.or(action), drag)
			}
			Err(error) => {
				ui.label(error);
				return;
			}
		};
		self.drag = active_drag;
		if let Some(action) = action {
			self.handle_action(action);
		}
	}

	fn handle_action(&mut self, action: Action) {
		match action {
			Action::Select { path, toggle } => {
				if toggle {
					if !self.selected.remove(&path) {
						self.selected.insert(path);
					}
				} else {
					self.selected = BTreeSet::from([path]);
				}
				self.position_draft = None;
			}
			Action::ClearSelection => self.selected.clear(),
			Action::Position { path, center } => {
				self.selected = BTreeSet::from([path.clone()]);
				self.set_position(path, [center.x(), center.y()]);
			}
			Action::Arrange(command) => self.arrange(command),
			Action::Navigate(path) => self.navigate(path),
		}
	}

	fn set_position(&mut self, target: VirtualPath, position: [f64; 2]) {
		self.apply(position_edit(target, position));
	}

	fn layout(&self) -> Result<ProjectFolder<'_>, String> {
		folder_layout(self.document.project(), &self.tree, &self.folder)
	}
}

pub(crate) enum Action {
	Select {
		path: VirtualPath,
		toggle: bool,
	},
	ClearSelection,
	Navigate(VirtualPath),
	Position {
		path: VirtualPath,
		center: FolderPoint,
	},
	Arrange(Arrangement),
}

fn position_edit(target: VirtualPath, position: [f64; 2]) -> ProjectEdit {
	let mut changes = ContentChanges::default();
	changes.position = FieldChange::Set(position);
	ProjectEdit::Modify { target, changes }
}

fn editable(project: &Project, path: &VirtualPath) -> bool {
	project
		.contents
		.iter()
		.any(|entry| entry.destination == *path)
}

fn folder_layout<'project>(
	project: &'project Project,
	tree: &'project Result<DeclaredTree, DeclaredTreeError>,
	folder: &VirtualPath,
) -> Result<ProjectFolder<'project>, String> {
	let tree = tree.as_ref().map_err(ToString::to_string)?;
	if tree.children(folder).len() > MAX_CANVAS_ENTRIES {
		return Err(format!(
			"The folder contains too many entries for the canvas ({MAX_CANVAS_ENTRIES} maximum)."
		));
	}
	tree.layout(project, folder)
		.map_err(|error| error.to_string())
}

fn parent(path: &str) -> &str {
	path.rsplit_once(PATH_SEPARATOR)
		.and_then(|(parent, _)| (!parent.is_empty()).then_some(parent))
		.unwrap_or(IMAGE_ROOT)
}

fn name(path: &VirtualPath) -> &str {
	path.components().last().unwrap_or(IMAGE_ROOT)
}
