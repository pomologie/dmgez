use std::collections::BTreeMap;

use dmgez_config::{
	Payload, Project, VirtualPath,
	project_edit::{
		ContentChanges, FieldChange, ProjectDocument, ProjectEdit, ProjectEditError, RevisionId,
	},
};
use dmgez_preview::{Drag, FolderPoint, ItemInput, PositionSource, ProjectFolder};
use eframe::egui::{self, Button, DragValue, Panel, ScrollArea, TextEdit, Ui};
use thiserror::Error;

const IMAGE_ROOT: &str = "/";
const PATH_SEPARATOR: char = '/';
const MAX_DISPLAY_ENTRIES: usize = 16_384;
const MAX_CANVAS_ENTRIES: usize = 512;
const MAX_LAYOUT_PATH_BYTES: usize = 4096;
const MIN_ZOOM_PERCENT: u32 = 25;
const MAX_ZOOM_PERCENT: u32 = 200;
const DEFAULT_ZOOM_PERCENT: u32 = 100;
const POSITION_SPEED: f64 = 0.25;
const LIST_WIDTH: f32 = 216.0;
const DETAILS_WIDTH: f32 = 226.0;

#[derive(Debug, Error)]
enum IndexError {
	#[error("the declared project tree exceeds the interactive preview entry or path limit")]
	Limit,
	#[error("the project contains an invalid preview path")]
	Path,
}

struct Node {
	path: VirtualPath,
	content: Option<usize>,
	directory: bool,
}

struct Index {
	folders: BTreeMap<String, Vec<Node>>,
}

impl Index {
	fn build(project: &Project) -> Result<Self, IndexError> {
		let mut nodes: BTreeMap<String, (Option<usize>, bool)> = BTreeMap::new();
		for (item, content) in project.contents.iter().enumerate() {
			let path = content.destination.as_str();
			if path.len() > MAX_LAYOUT_PATH_BYTES {
				return Err(IndexError::Limit);
			}
			nodes.insert(
				path.to_owned(),
				(Some(item), matches!(content.payload, Payload::Directory)),
			);
			let mut folder = parent(path);
			while folder != IMAGE_ROOT {
				nodes.entry(folder.to_owned()).or_insert((None, true));
				if nodes.len() > MAX_DISPLAY_ENTRIES {
					return Err(IndexError::Limit);
				}
				folder = parent(folder);
			}
		}
		if nodes.len() > MAX_DISPLAY_ENTRIES {
			return Err(IndexError::Limit);
		}
		let mut folders = BTreeMap::<String, Vec<Node>>::new();
		for (path, (content, directory)) in nodes {
			let destination = path.parse().map_err(|_| IndexError::Path)?;
			folders
				.entry(parent(&path).to_owned())
				.or_default()
				.push(Node {
					path: destination,
					content,
					directory,
				});
		}
		Ok(Self { folders })
	}

	fn children(&self, folder: &VirtualPath) -> &[Node] {
		self.folders.get(folder.as_str()).map_or(&[], Vec::as_slice)
	}
}

pub(crate) struct ProjectTab {
	pub document: ProjectDocument,
	saved: RevisionId,
	index: Result<Index, IndexError>,
	folder: VirtualPath,
	selected: Option<VirtualPath>,
	zoom: u32,
	show_hidden: bool,
	snap: bool,
	image_name: String,
	position_draft: Option<(VirtualPath, [f64; 2])>,
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
			index: Index::build(project),
			folder: IMAGE_ROOT.parse().expect("root is a valid virtual path"),
			selected: None,
			zoom: DEFAULT_ZOOM_PERCENT,
			show_hidden: false,
			snap: false,
			image_name: project.image.name.as_str().to_owned(),
			position_draft: None,
			drag: None,
			error: None,
			document,
		}
	}

	pub(crate) fn is_dirty(&self) -> bool {
		self.document.revision() != self.saved
	}

	pub(crate) fn mark_saved(&mut self, revision: RevisionId) {
		self.saved = revision;
	}

	fn apply(&mut self, edit: ProjectEdit) {
		let result = self.document.apply(&[edit]);
		self.refresh_after_edit(result);
	}

	fn refresh_after_edit(&mut self, result: Result<bool, ProjectEditError>) {
		match result {
			Ok(true) => {
				self.image_name = self.document.project().image.name.as_str().to_owned();
				self.position_draft = None;
				self.error = None;
			}
			Ok(false) => {}
			Err(error) => self.error = Some(error.to_string()),
		}
	}

	pub(crate) fn undo(&mut self) {
		let result = self.document.undo();
		self.refresh_after_edit(result);
	}

	pub(crate) fn redo(&mut self) {
		let result = self.document.redo();
		self.refresh_after_edit(result);
	}

	fn navigate(&mut self, folder: VirtualPath) {
		self.folder = folder;
		self.selected = None;
		self.position_draft = None;
		self.drag = None;
	}

	pub(crate) fn ui(&mut self, ui: &mut Ui) {
		if let Err(error) = &self.index {
			ui.label(error.to_string());
			return;
		}
		let mut action = None;
		Panel::left("project_entries")
			.default_size(LIST_WIDTH)
			.show(ui, |ui| {
				action = self.entries(ui);
			});
		if let Some(action) = action {
			match action {
				Action::Select(path) => self.selected = Some(path),
				Action::Navigate(path) => self.navigate(path),
				Action::Position { .. } => {}
			}
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

	fn entries(&self, ui: &mut Ui) -> Option<Action> {
		let index = self.index.as_ref().ok()?;
		ui.heading("Contents");
		let mut action = None;
		ui.horizontal(|ui| {
			if ui
				.add_enabled(!self.folder.is_root(), Button::new("Up"))
				.clicked()
				&& let Ok(folder) = parent(self.folder.as_str()).parse()
			{
				action = Some(Action::Navigate(folder));
			}
			ui.label(self.folder.as_str());
		});
		let nodes = index.children(&self.folder);
		let row_height = ui.spacing().interact_size.y;
		ScrollArea::vertical().show_rows(ui, row_height, nodes.len(), |ui, rows| {
			for row in rows {
				let node = &nodes[row];
				let name = name(&node.path);
				let selected = self.selected.as_ref() == Some(&node.path);
				ui.horizontal(|ui| {
					if ui.selectable_label(selected, name).clicked() {
						action = Some(Action::Select(node.path.clone()));
					}
					if node.directory && ui.small_button("Open").clicked() {
						action = Some(Action::Navigate(node.path.clone()));
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
		let Some(path) = self.selected.clone() else {
			return;
		};
		ui.strong(name(&path));
		ui.small(path.as_str());
		let editable = self.editable(&path);
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
		ui.horizontal(|ui| {
			ui.heading("Layout");
			ui.add(
				egui::Slider::new(&mut self.zoom, MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).suffix("%"),
			);
			ui.checkbox(&mut self.show_hidden, "Show hidden");
			ui.checkbox(&mut self.snap, "Snap to grid");
		});
		if let Some(error) = &self.error {
			ui.label(error);
		}
		let (action, active_drag) = match self.layout() {
			Ok(layout) => {
				let scale = f64::from(self.zoom) / f64::from(DEFAULT_ZOOM_PERCENT);
				let view = crate::canvas::View {
					selected: self.selected.as_ref(),
					show_hidden: self.show_hidden,
					snap: self.snap,
					scale,
					drag: self.drag.as_ref(),
				};
				crate::canvas::show(ui, &layout, view, |path| self.editable(path))
			}
			Err(error) => {
				ui.label(error);
				return;
			}
		};
		self.drag = active_drag;
		match action {
			Some(Action::Select(path)) => self.selected = Some(path),
			Some(Action::Position { path, center }) => {
				self.selected = Some(path.clone());
				self.set_position(path, [center.x(), center.y()]);
			}
			Some(Action::Navigate(path)) => self.navigate(path),
			None => {}
		}
	}

	fn set_position(&mut self, target: VirtualPath, position: [f64; 2]) {
		let mut changes = ContentChanges::default();
		changes.position = FieldChange::Set(position);
		self.apply(ProjectEdit::Modify { target, changes });
	}

	fn editable(&self, path: &VirtualPath) -> bool {
		self.document
			.project()
			.contents
			.iter()
			.any(|entry| entry.destination == *path)
	}

	fn layout(&self) -> Result<ProjectFolder<'_>, String> {
		let index = self.index.as_ref().map_err(ToString::to_string)?;
		let children = index.children(&self.folder);
		if children.len() > MAX_CANVAS_ENTRIES {
			return Err(format!(
				"The folder contains too many entries for the canvas ({MAX_CANVAS_ENTRIES} maximum)."
			));
		}
		let project = self.document.project();
		let inputs: Vec<_> = children
			.iter()
			.map(|node| {
				node.content.map_or(
					ItemInput {
						destination: &node.path,
						position: None,
						hidden: false,
						hide_extension: false,
					},
					|item| ItemInput::from(&project.contents[item]),
				)
			})
			.collect();
		ProjectFolder::from_project(project, &self.folder, &inputs)
			.map_err(|error| error.to_string())
	}
}

pub(crate) enum Action {
	Select(VirtualPath),
	Navigate(VirtualPath),
	Position {
		path: VirtualPath,
		center: FolderPoint,
	},
}

fn parent(path: &str) -> &str {
	path.rsplit_once(PATH_SEPARATOR)
		.and_then(|(parent, _)| (!parent.is_empty()).then_some(parent))
		.unwrap_or(IMAGE_ROOT)
}

fn name(path: &VirtualPath) -> &str {
	path.components().last().unwrap_or(IMAGE_ROOT)
}

#[cfg(test)]
mod tests {
	use super::*;

	const SAMPLE: &str = "# original\nschema_version=1\n[image]\nname='Test'\noutput='test.dmg'\n[[contents]]\nsource='unopened.txt'\ndestination='/Docs/Test.txt'\nposition=[20,30]\n";

	#[test]
	fn navigates_implied_folder_and_undoes_a_saved_position() {
		let mut tab = ProjectTab::new(ProjectDocument::open(SAMPLE).unwrap());
		assert_eq!(
			tab.layout().unwrap().items()[0].destination.as_str(),
			"/Docs"
		);
		tab.navigate("/Docs".parse().unwrap());
		let target = tab.layout().unwrap().items()[0].destination.clone();
		tab.set_position(target, [98.0, 65.0]);
		assert!(tab.is_dirty());
		assert_eq!(tab.layout().unwrap().items()[0].center.x(), 98.0);
		assert!(tab.document.source().starts_with("# original"));
		tab.undo();
		assert!(!tab.is_dirty());
		tab.redo();
		let revision = tab.document.revision();
		tab.mark_saved(revision);
		assert!(!tab.is_dirty());
		assert_eq!(tab.layout().unwrap().items()[0].center.y(), 65.0);
	}
}
