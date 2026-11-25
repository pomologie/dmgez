mod app;
mod canvas;
mod document;
mod import;
mod inspector;
mod layout_lint;
mod project;

use std::path::PathBuf;

use eframe::egui;

const APP_NAME: &str = "dmgez";
const INITIAL_SIZE: [f32; 2] = [1140.0, 740.0];
const MINIMUM_SIZE: [f32; 2] = [760.0, 440.0];

fn main() -> eframe::Result {
	let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_inner_size(INITIAL_SIZE)
			.with_min_inner_size(MINIMUM_SIZE),
		..Default::default()
	};
	eframe::run_native(
		APP_NAME,
		options,
		Box::new(move |context| Ok(Box::new(app::App::new(&context.egui_ctx, initial_path)))),
	)
}
