use dmgez_vfs::volume::NodeKind;
use eframe::egui::{self, Label, RichText, Ui};

use crate::{
	image::browse::{
		Details, Request,
		content::{Extent, Preview},
	},
	inspector::Action,
};

pub(super) fn show(ui: &mut Ui, details: &Details, idle: bool) -> Option<Action> {
	let Some(preview) = &details.text else {
		return (details.metadata.kind == NodeKind::RegularFile
			&& ui
				.add_enabled(idle, egui::Button::new("Preview text"))
				.clicked())
		.then(|| {
			Action::Browse(Request::Text {
				folder: details.folder.clone(),
				name: details.name.clone(),
			})
		});
	};
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
	None
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
