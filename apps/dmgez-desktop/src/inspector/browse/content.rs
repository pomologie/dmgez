use dmgez_vfs::volume::NodeKind;
use eframe::egui::{self, Label, RichText, Ui};

use crate::{
	image::browse::{
		Details, Request,
		content::{Extent, Preview, png},
	},
	inspector::Action,
};

const PNG_ALT_TEXT: &str = "Decoded PNG file preview";
const MAX_IMAGE_HEIGHT: f32 = 400.0;

pub(super) fn show(ui: &mut Ui, details: &Details, idle: bool) -> Option<Action> {
	let action = (details.metadata.kind == NodeKind::RegularFile)
		.then(|| ui.horizontal_wrapped(|ui| actions(ui, details, idle)).inner)
		.flatten();
	if let Some(preview) = &details.text {
		show_text_preview(ui, preview);
	}
	if let Some(preview) = details.png.as_deref() {
		show_png(ui, preview);
	}
	action
}

fn actions(ui: &mut Ui, details: &Details, idle: bool) -> Option<Action> {
	let text = ui
		.add_enabled(
			idle && details.text.is_none(),
			egui::Button::new("Preview text"),
		)
		.clicked();
	let png = ui
		.add_enabled(
			idle && details.png.is_none(),
			egui::Button::new("Preview PNG"),
		)
		.clicked();
	if text {
		return Some(Action::Browse(Request::Text {
			folder: details.folder.clone(),
			name: details.name.clone(),
		}));
	}
	png.then(|| {
		Action::Browse(Request::Png {
			folder: details.folder.clone(),
			name: details.name.clone(),
		})
	})
}

fn show_text_preview(ui: &mut Ui, preview: &Preview) {
	ui.separator();
	ui.strong("Text preview");
	match preview {
		Preview::Text { value, extent } => show_text(ui, value, extent),
		Preview::Binary => {
			ui.label("The sampled bytes contain non-text control characters.");
		}
		Preview::UnsupportedEncoding => {
			ui.label("The sampled bytes are not valid UTF-8.");
		}
		Preview::UnsupportedKind => {
			ui.label("Text preview is available for regular files.");
		}
		Preview::AbsentDataFork => {
			ui.label("This file has no data fork.");
		}
		Preview::AllocationUnavailable => {
			ui.label("Memory for the text preview is unavailable.");
		}
	}
}

fn show_png(ui: &mut Ui, preview: &png::Preview) {
	ui.separator();
	ui.strong("PNG preview");
	match preview {
		Ok(image) => {
			let [width, height] = image.size;
			ui.label(format!("{width} × {height} pixels"));
			ui.add(
				egui::Image::new(&image.texture)
					.max_width(ui.available_width())
					.max_height(MAX_IMAGE_HEIGHT)
					.alt_text(PNG_ALT_TEXT),
			);
		}
		Err(issue) => {
			ui.label(issue.to_string());
		}
	}
}

fn show_text(ui: &mut Ui, value: &str, extent: &Extent) {
	match extent {
		Extent::Complete if value.is_empty() => {
			ui.label("Empty file");
			return;
		}
		Extent::Complete => {
			ui.label("Complete UTF-8 text");
		}
		Extent::Prefix { file_bytes } => {
			ui.label(format!("Showing {} of {file_bytes} bytes", value.len()));
		}
	}
	ui.add(
		Label::new(RichText::new(value).monospace())
			.wrap()
			.selectable(true),
	);
}
