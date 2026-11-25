//! Find controls and bounded results for the current image folder.

use dmgez_hfsplus_read::browse::NameFragment;
use dmgez_inspector::volume_queries::SearchExtent;
use eframe::egui::{Button, Key, TextEdit, Ui};

use crate::image::browse::{Location, Request, search::State};

const SEARCH_LABEL: &str = "Find in folder";
const SEARCH_BUTTON: &str = "Search";

pub(super) fn show(ui: &mut Ui, location: &Location, state: &mut State) -> (bool, Option<Request>) {
	let request = ui
		.horizontal_wrapped(|ui| {
			let label = ui.label(SEARCH_LABEL);
			let response = ui
				.add(TextEdit::singleline(&mut state.query).char_limit(NameFragment::MAX_UNITS))
				.labelled_by(label.id);
			if response.changed() {
				state.report = None;
			}
			let entered = response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
			let clicked = ui
				.add_enabled(!state.query.is_empty(), Button::new(SEARCH_BUTTON))
				.clicked();
			if ui
				.add_enabled(!state.query.is_empty(), Button::new("Clear search"))
				.clicked()
			{
				state.clear();
				return None;
			}
			(ui.is_enabled() && !state.query.is_empty() && (clicked || entered)).then(|| {
				Request::Search {
					location: location.clone(),
					query: state.query.clone(),
				}
			})
		})
		.inner;
	let Some(report) = &state.report else {
		return (false, request);
	};
	ui.label(format!(
		"{} matches · {} entries searched",
		report.entries.len(),
		report.scanned_entries
	));
	match report.extent {
		SearchExtent::Complete => {}
		SearchExtent::ScanLimit => {
			ui.label("Search reached the directory entry limit. More matches may exist.");
		}
		SearchExtent::MatchLimit => {
			ui.label("Search reached the result limit. Enter a more specific name fragment.");
		}
	}
	(
		true,
		request.or_else(|| super::show_entries(ui, location, &report.entries)),
	)
}
