use std::{
	mem,
	path::{Path, PathBuf},
};

use dmgez_config::{VirtualPath, project_edit::ProjectEdit};
use eframe::egui::Context;

use crate::{
	document::{Document, Opened},
	import,
};

use super::{App, Completion};

impl App {
	pub(super) fn add_files(
		&mut self,
		ctx: &Context,
		pick: impl FnOnce(&Path) -> Option<Vec<PathBuf>> + Send + 'static,
	) {
		self.import_batch(ctx, move |directory, folder| {
			pick(directory)
				.filter(|paths| !paths.is_empty())
				.map(|paths| import::prepare(directory, folder, paths))
		});
	}

	pub(super) fn import_dropped_files(&mut self, ctx: &Context) {
		let files = ctx.input_mut(|input| mem::take(&mut input.raw.dropped_files));
		if files.is_empty()
			|| self.worker.is_some()
			|| self.confirm.is_some()
			|| self.save_copy
			|| self.palette.is_some()
			|| self.closing
			|| ctx.input(|input| input.viewport().close_requested())
		{
			return;
		}
		self.import_batch(ctx, move |directory, folder| {
			Some(import::prepare(
				directory,
				folder,
				files.iter().map(|file| file.path()),
			))
		});
	}

	fn import_batch(
		&mut self,
		ctx: &Context,
		prepare: impl FnOnce(&Path, &VirtualPath) -> Option<Result<Vec<ProjectEdit>, import::Error>>
		+ Send
		+ 'static,
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
				prepare(&directory, &folder).map_or(Completion::Cancelled, |edits| {
					Completion::Added { revision, edits }
				})
			});
		}
	}
}
