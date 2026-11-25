use dmgez_preview::{
	ItemVisibility,
	actual::{BackgroundStatus, LayoutStatus, StoreStatus},
};
use eframe::egui::{self, Align, Image, Rect, Sense, StrokeKind, Ui};

use crate::{
	image::browse::{Location, Request, label, preview::Presented},
	inspector::{Action, field},
};

const PREVIEW_ALT_TEXT: &str = "Icon layout rendered from the disk image";
const ICON_ID: &str = "actual_preview_icon";

#[cfg(test)]
mod tests;

pub(super) fn show(
	ui: &mut Ui,
	location: &Location,
	presented: Option<&Presented>,
) -> Option<Action> {
	let Some(presented) = presented else {
		return ui
			.button("Preview folder")
			.clicked()
			.then(|| preview_action(ui, location, ItemVisibility::Visible));
	};
	let navigation = ui
		.horizontal_wrapped(|ui| {
			let list = ui
				.button("Show catalog list")
				.clicked()
				.then_some(Action::ShowList);
			let mut hidden = presented.visibility == ItemVisibility::IncludeHidden;
			if ui.checkbox(&mut hidden, "Show hidden items").changed() {
				let visibility = if hidden {
					ItemVisibility::IncludeHidden
				} else {
					ItemVisibility::Visible
				};
				return Some(preview_action(ui, location, visibility));
			}
			list
		})
		.inner;
	let coverage = presented.coverage;
	ui.strong(layout_status(coverage.layout));
	field(ui, "Finder metadata", store_status(coverage.store));
	field(ui, "Background", background_status(coverage.background));
	ui.label(format!(
		"{} stored positions · {} fallback positions",
		coverage.observed_positions, coverage.fallback_positions,
	));
	let texture = &presented.texture;
	let image = ui.add(Image::new((texture.id(), texture.size_vec2())).alt_text(PREVIEW_ALT_TEXT));
	let icon = interact_icons(ui, image.rect, location, presented);
	navigation.or(icon)
}

fn preview_action(ui: &Ui, location: &Location, visibility: ItemVisibility) -> Action {
	Action::Browse(Request::Preview {
		location: location.clone(),
		max_side: ui.input(|input| input.max_texture_side),
		visibility,
	})
}

fn interact_icons(
	ui: &mut Ui,
	image: Rect,
	location: &Location,
	presented: &Presented,
) -> Option<Action> {
	let mut picked = None;
	for (index, icon) in presented.icons.iter().enumerate() {
		let bounds = icon.bounds.translate(image.min.to_vec2());
		let enabled = icon.can_activate(location);
		let id = ui.make_persistent_id((ICON_ID, presented.texture.id(), index));
		let response = ui.interact(
			bounds,
			id,
			if enabled {
				Sense::click()
			} else {
				Sense::hover()
			},
		);
		response.widget_info(|| {
			let action = if icon.opens_folder() {
				"Open folder"
			} else {
				"Inspect"
			};
			egui::WidgetInfo::labeled(
				egui::Role::Button,
				enabled && response.enabled(),
				format!("{action} {}", label(&icon.name)),
			)
		});
		if response.gained_focus() {
			response.scroll_to_me(Some(Align::Center));
		}
		if (response.has_focus() || response.hovered()) && enabled && response.enabled() {
			ui.painter().rect_stroke(
				bounds,
				0.0,
				ui.visuals().selection.stroke,
				StrokeKind::Inside,
			);
		}
		if response.clicked() && enabled {
			picked = icon.request(location).map(Action::Browse);
		}
	}
	picked
}

fn store_status(status: StoreStatus) -> &'static str {
	match status {
		StoreStatus::Observed => "Read from the image",
		StoreStatus::Absent => "Absent",
		StoreStatus::Unavailable => "Unreadable",
		StoreStatus::Unsupported => "Unsupported store",
		StoreStatus::Malformed => "Malformed store",
		_ => "Unknown",
	}
}

fn layout_status(status: LayoutStatus) -> &'static str {
	match status {
		LayoutStatus::ObservedIcon => "Stored Finder icon layout",
		LayoutStatus::PartialIcon => "Finder icon layout with fallback positions",
		LayoutStatus::GenericNoStore => "Generic icon layout",
		LayoutStatus::GenericOtherView => "Generic icon layout for a different stored view",
		LayoutStatus::GenericAutomaticArrangement => {
			"Generic icon layout for a stored automatic arrangement"
		}
		LayoutStatus::GenericInvalidSettings => "Generic icon layout for invalid stored settings",
		_ => "Unknown layout",
	}
}

fn background_status(status: BackgroundStatus) -> &'static str {
	match status {
		BackgroundStatus::Default => "Default",
		BackgroundStatus::ObservedColor => "Stored solid color",
		BackgroundStatus::ObservedPng => "Image asset from the volume",
		BackgroundStatus::UnavailableImage => "Default because the image is unavailable",
		BackgroundStatus::UnsupportedSetting => "Default because the stored setting is unsupported",
		BackgroundStatus::InvalidSetting => "Default because the stored setting is invalid",
		BackgroundStatus::UnsupportedImage => "Default because the image format is unsupported",
		BackgroundStatus::MalformedImage => "Default because the image is malformed",
		_ => "Unknown",
	}
}
