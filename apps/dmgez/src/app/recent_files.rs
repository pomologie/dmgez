use std::{
	ffi::OsString,
	path::{Path, PathBuf},
};

use eframe::egui::{self, Context, Ui};
use serde::{Deserialize, Serialize};

use crate::document::{self, Kind, Opened};

use super::{App, Completion, Pending, open_outcome};

mod storage;
#[cfg(test)]
mod tests;

pub(super) use storage::{Error, Store};

const MAX_ENTRIES: usize = 12;
const MAX_PATH_BYTES: usize = 4096;
const SCHEMA_VERSION: u32 = 1;
const MENU_LABEL: &str = "Recent files";
const REMEMBER_LABEL: &str = "Remember recent files";
const CLEAR_LABEL: &str = "Clear recent files";
const MENU_WIDTH: f32 = 420.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
	path: OsString,
	kind: Kind,
}

impl Entry {
	fn from_opened(opened: &Opened) -> Option<Self> {
		Self::valid_path(&opened.path).then(|| Self {
			path: opened.path.as_os_str().to_owned(),
			kind: opened.document.kind(),
		})
	}

	fn path(&self) -> &Path {
		Path::new(&self.path)
	}

	fn valid_path(path: &Path) -> bool {
		let encoded = path.as_os_str().as_encoded_bytes();
		path.is_absolute() && encoded.len() <= MAX_PATH_BYTES && !encoded.contains(&0)
	}

	fn open(self) -> Completion {
		match document::open_as(PathBuf::from(self.path), self.kind) {
			Ok(opened) => Completion::Opened(opened),
			Err(error) => Completion::Failed(error),
		}
	}
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preferences {
	schema_version: u32,
	remember: bool,
	entries: Vec<Entry>,
}

impl Default for Preferences {
	fn default() -> Self {
		Self {
			schema_version: SCHEMA_VERSION,
			remember: true,
			entries: Vec::new(),
		}
	}
}

impl Preferences {
	fn valid(&self) -> bool {
		self.schema_version == SCHEMA_VERSION
			&& self.entries.len() <= MAX_ENTRIES
			&& (self.remember || self.entries.is_empty())
			&& self.entries.iter().enumerate().all(|(index, entry)| {
				Entry::valid_path(entry.path())
					&& self.entries[..index]
						.iter()
						.all(|other| other.path != entry.path)
			})
	}

	fn apply(&mut self, change: Change) -> bool {
		match change {
			Change::Opened(entry) if self.remember && self.entries.first() != Some(&entry) => {
				self.entries.retain(|other| other.path != entry.path);
				self.entries.truncate(MAX_ENTRIES - 1);
				self.entries.insert(0, entry);
				true
			}
			Change::Remember(remember) if remember != self.remember => {
				self.remember = remember;
				if !remember {
					self.entries.clear();
				}
				true
			}
			Change::Clear if !self.entries.is_empty() => {
				self.entries.clear();
				true
			}
			_ => false,
		}
	}
}

pub(super) enum Change {
	Opened(Entry),
	Remember(bool),
	Clear,
}

enum Action {
	Open(Entry),
	Update(Change),
}

pub(super) struct Loaded {
	store: Store,
	preferences: Preferences,
}

#[derive(Default)]
pub(super) struct RecentFiles {
	loaded: Option<Loaded>,
	pub(super) error: Option<String>,
}

impl Loaded {
	fn open(store: Store) -> Result<Self, Error> {
		let preferences = store.load()?;
		Ok(Self { store, preferences })
	}

	fn menu(&self, ui: &mut Ui) -> Option<Action> {
		ui.set_max_width(MENU_WIDTH);
		if self.preferences.entries.is_empty() {
			let message = if self.preferences.remember {
				"No recent files"
			} else {
				"Recent files are turned off"
			};
			ui.label(message);
		}
		for entry in &self.preferences.entries {
			let path = entry.path().display().to_string();
			if ui
				.add(egui::Button::new(&path).truncate())
				.on_hover_text(path)
				.clicked()
			{
				ui.close();
				return Some(Action::Open(entry.clone()));
			}
		}
		ui.separator();
		let mut remember = self.preferences.remember;
		if ui.checkbox(&mut remember, REMEMBER_LABEL).changed() {
			return Some(Action::Update(Change::Remember(remember)));
		}
		if ui
			.add_enabled(
				!self.preferences.entries.is_empty(),
				egui::Button::new(CLEAR_LABEL),
			)
			.clicked()
		{
			return Some(Action::Update(Change::Clear));
		}
		None
	}
}

impl App {
	pub(crate) fn with_recent_files(ctx: &Context, initial_path: Option<PathBuf>) -> Self {
		let mut app = Self::new(ctx, None);
		app.start(ctx, move || Completion::RecentsLoaded {
			result: Store::local().and_then(Loaded::open),
			initial_path,
		});
		app
	}

	pub(super) fn finish_recent_load(
		&mut self,
		ctx: &Context,
		result: Result<Loaded, Error>,
		initial_path: Option<PathBuf>,
	) {
		match result {
			Ok(loaded) => self.recent.loaded = Some(loaded),
			Err(error) => {
				self.recent.error = Some(format!("Could not load recent files. {error}"));
			}
		}
		if let Some(path) = initial_path {
			self.start(ctx, move || open_outcome(path));
		}
	}

	pub(super) fn finish_recent_update(&mut self, result: Result<Preferences, Error>) {
		match result {
			Ok(preferences) => {
				if let Some(loaded) = &mut self.recent.loaded {
					loaded.preferences = preferences;
				}
				self.recent.error = None;
			}
			Err(error) => {
				self.recent.error = Some(format!("Could not update recent files. {error}"));
			}
		}
	}

	pub(super) fn remember_open(&mut self, ctx: &Context, opened: &Opened) {
		if self
			.recent
			.loaded
			.as_ref()
			.is_some_and(|loaded| loaded.preferences.remember)
			&& let Some(entry) = Entry::from_opened(opened)
		{
			self.update_recents(ctx, Change::Opened(entry));
		}
	}

	fn update_recents(&mut self, ctx: &Context, change: Change) {
		if let Some(loaded) = &self.recent.loaded {
			let store = loaded.store.clone();
			self.start(ctx, move || {
				Completion::RecentsUpdated(store.update(change))
			});
		}
	}

	pub(super) fn recent_menu(&mut self, ui: &mut Ui) {
		let action = ui
			.menu_button(MENU_LABEL, |ui| match &self.recent.loaded {
				Some(loaded) => loaded.menu(ui),
				None => {
					ui.label("Recent files unavailable");
					None
				}
			})
			.inner
			.flatten();
		match action {
			Some(Action::Open(entry)) if self.is_dirty() => {
				self.confirm = Some(Pending::OpenRecent(entry));
			}
			Some(Action::Open(entry)) => self.open_recent(ui.ctx(), entry),
			Some(Action::Update(change)) => self.update_recents(ui.ctx(), change),
			None => {}
		}
	}

	pub(super) fn open_recent(&mut self, ctx: &Context, entry: Entry) {
		self.start(ctx, move || entry.open());
	}
}
