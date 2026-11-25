use eframe::egui::{self, accesskit};

use crate::{
	image::ImageTab,
	inspector::{self, Action},
};

pub(super) fn activate(tab: &mut ImageTab, title: &str) -> Action {
	let context = egui::Context::default();
	context.enable_accesskit();
	let viewport = egui::Rect::from_min_size(
		egui::Pos2::ZERO,
		egui::vec2(crate::INITIAL_SIZE[0], 1_600.0),
	);
	let mut initial = context.run_ui(
		egui::RawInput {
			screen_rect: Some(viewport),
			..Default::default()
		},
		|ui| assert!(inspector::show(ui, tab, true).is_none()),
	);
	let update = initial.platform_output.accesskit_update.take().unwrap();
	initial.drop_without_applying_deltas();
	let target_node = update
		.nodes
		.into_iter()
		.find(|(_, node)| {
			node.label() == Some(title) && node.supports_action(accesskit::Action::Click)
		})
		.unwrap_or_else(|| panic!("missing reachable {title} control"))
		.0;
	let mut action = None;
	context
		.run_ui(
			egui::RawInput {
				screen_rect: Some(viewport),
				events: vec![egui::Event::AccessKitActionRequest(
					accesskit::ActionRequest {
						action: accesskit::Action::Click,
						target_tree: accesskit::TreeId::ROOT,
						target_node,
						data: None,
					},
				)],
				..Default::default()
			},
			|ui| {
				if let Some(emitted) = inspector::show(ui, tab, true) {
					action = Some(emitted);
				}
			},
		)
		.drop_without_applying_deltas();
	action.unwrap_or_else(|| panic!("no action emitted from {title}"))
}
