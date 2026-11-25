use dmgez_preview::actual::{BackgroundStatus, LayoutStatus, StoreStatus};
use eframe::egui::{Image, Ui};

use crate::{
	image::browse::{Location, Request, preview::Presented},
	inspector::{Action, field},
};

const PREVIEW_ALT_TEXT: &str = "Icon layout rendered from the disk image";

pub(super) fn show(
	ui: &mut Ui,
	location: &Location,
	presented: Option<&Presented>,
) -> Option<Action> {
	let Some(presented) = presented else {
		return ui.button("Preview folder").clicked().then(|| {
			Action::Browse(Request::Preview {
				location: location.clone(),
				max_side: ui.input(|input| input.max_texture_side),
			})
		});
	};
	let list = ui
		.button("Show catalog list")
		.clicked()
		.then_some(Action::ShowList);
	let coverage = presented.coverage;
	ui.strong(layout_status(coverage.layout));
	field(ui, "Finder metadata", store_status(coverage.store));
	field(ui, "Background", background_status(coverage.background));
	ui.label(format!(
		"{} stored positions · {} fallback positions",
		coverage.observed_positions, coverage.fallback_positions,
	));
	let texture = &presented.texture;
	ui.add(Image::new((texture.id(), texture.size_vec2())).alt_text(PREVIEW_ALT_TEXT));
	list
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
