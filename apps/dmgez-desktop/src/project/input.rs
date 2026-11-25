use dmgez_config::VirtualPath;
use eframe::egui::{Event, EventFilter, Key, Modifiers, Response, Ui};

use super::{Action, arrangement::Arrangement};

const NUDGE_POINTS: f64 = 1.0;
const LARGE_NUDGE_POINTS: f64 = 10.0;

pub(crate) fn select(ui: &Ui, path: &VirtualPath) -> Action {
	let toggle = ui.input(|input| input.modifiers.command || input.modifiers.shift);
	Action::Select {
		path: path.clone(),
		toggle,
	}
}

pub(crate) fn nudge(ui: &Ui, response: &Response) -> Option<Action> {
	if !response.enabled() || !response.has_focus() {
		return None;
	}
	ui.memory_mut(|memory| {
		memory.set_focus_lock_filter(
			response.id,
			EventFilter {
				horizontal_arrows: true,
				vertical_arrows: true,
				..Default::default()
			},
		);
	});
	let mut delta = [0.0, 0.0];
	ui.input_mut(|input| {
		input.events.retain(|event| {
			let Event::Key {
				key,
				pressed: true,
				repeat,
				modifiers,
				..
			} = event
			else {
				return true;
			};
			let points = if modifiers.shift_only() {
				LARGE_NUDGE_POINTS
			} else if modifiers.matches_exact(Modifiers::NONE) {
				NUDGE_POINTS
			} else {
				return true;
			};
			let [x, y] = match key {
				Key::ArrowLeft => [-points, 0.0],
				Key::ArrowRight => [points, 0.0],
				Key::ArrowUp => [0.0, -points],
				Key::ArrowDown => [0.0, points],
				_ => return true,
			};
			if !repeat {
				delta[0] += x;
				delta[1] += y;
			}
			false
		})
	});
	(delta != [0.0, 0.0]).then_some(Action::Arrange(Arrangement::Nudge(delta)))
}
