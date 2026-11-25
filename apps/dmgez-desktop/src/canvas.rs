use std::collections::BTreeSet;

use dmgez_config::{Background, LabelPosition, VirtualPath, parse_solid_background_color};
use dmgez_preview::{
	CanvasPoint, Drag, FolderPoint, ItemVisibility, PositionSource, ProjectFolder, Viewport,
};
use eframe::egui::{
	Align2, Color32, FontId, Pos2, Rect, Role, ScrollArea, Sense, Stroke, StrokeKind, Ui,
	WidgetInfo, pos2, vec2,
};

use crate::project::{Action, DragEdit, input};

const MAX_CANVAS_POINTS: f64 = 32_768.0;
const CANVAS_EDGE: Color32 = Color32::from_rgb(184, 199, 199);
const CANVAS_BACKGROUND: Color32 = Color32::from_rgb(245, 249, 248);
const ITEM_SELECTED: Color32 = Color32::from_rgb(0, 111, 107);
const ITEM_SURFACE: Color32 = Color32::from_rgb(215, 234, 229);
const ITEM_EDGE: Color32 = Color32::from_rgb(103, 132, 129);
const ITEM_FOREGROUND: Color32 = Color32::from_rgb(34, 56, 60);
const LINT_WARNING: Color32 = Color32::from_rgb(151, 67, 13);
const LINT_BORDER_WIDTH: f32 = 2.0;
const LINT_MARK_RADIUS: f32 = 8.0;
const LINT_MARK_FONT_SIZE: f32 = 12.0;
const LINT_MARK_TEXT: &str = "!";
const ITEM_CORNER: u8 = 8;
const LABEL_MARGIN: f32 = 5.0;
const MAX_LABEL_POINTS: f64 = 36.0;
const FIT_PADDING: f32 = 26.0;

pub(crate) struct View<'a> {
	pub selected: &'a BTreeSet<VirtualPath>,
	pub nudge_enabled: bool,
	pub show_hidden: bool,
	pub snap: bool,
	pub scale: f64,
	pub drag: Option<&'a DragEdit>,
	pub lint_warnings: Option<&'a BTreeSet<String>>,
}

pub(crate) fn show(
	ui: &mut Ui,
	layout: &ProjectFolder<'_>,
	view: View<'_>,
	editable: impl Fn(&VirtualPath) -> bool,
) -> (Option<Action>, Option<DragEdit>) {
	let style = layout.style();
	let width = style.window_size.width() * view.scale;
	let height = style.window_size.height() * view.scale;
	if [width, height]
		.into_iter()
		.any(|side| side > MAX_CANVAS_POINTS || !side.is_finite())
	{
		ui.label("The window exceeds the canvas limit at this zoom.");
		return (None, None);
	}
	let mut action = None;
	let mut drag = view.drag.map(|active| DragEdit {
		path: active.path.clone(),
		drag: active.drag,
		center: active.center,
	});
	ScrollArea::both().id_salt("project_canvas").show(ui, |ui| {
		ui.add_space(FIT_PADDING);
		let canvas_size = vec2(width as f32, height as f32);
		let (rect, response) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
		response
			.widget_info(|| WidgetInfo::labeled(Role::Canvas, response.enabled(), "Icon canvas"));
		let Some(origin) = canvas_point(rect.min) else {
			return;
		};
		let Ok(viewport) = style.viewport(origin, view.scale) else {
			return;
		};
		let visibility = if view.show_hidden {
			ItemVisibility::IncludeHidden
		} else {
			ItemVisibility::Visible
		};
		let hit = |point| {
			canvas_point(point)
				.and_then(|point| layout.hit_test(point, viewport, visibility).ok().flatten())
		};
		if response.clicked()
			&& let Some(point) = response.interact_pointer_pos()
		{
			action = Some(hit(point).map_or(Action::ClearSelection, |item| {
				input::select(ui, item.destination)
			}));
			response.request_focus();
		}
		if response.drag_started()
			&& let Some(start) = ui.input(|input| input.pointer.press_origin())
			&& let Some(item) = hit(start)
			&& item.position_source != PositionSource::FinderFolder
			&& editable(item.destination)
			&& let Some(press) = canvas_point(start)
			&& let Ok(started) = Drag::begin(item.center, press, viewport)
		{
			drag = Some(DragEdit {
				path: item.destination.clone(),
				drag: started,
				center: item.center,
			});
			action = Some(Action::Select {
				path: item.destination.clone(),
				toggle: false,
			});
			response.request_focus();
		}
		let grid = view.snap.then(|| style.snap_grid().ok()).flatten();
		if let Some(active) = &mut drag
			&& let Some(point) = response.interact_pointer_pos().and_then(canvas_point)
			&& let Ok(center) = active.drag.center_at(point, viewport, grid)
		{
			active.center = center;
		}
		if response.drag_stopped()
			&& let Some(active) = drag.take()
		{
			action = Some(Action::Position {
				path: active.path,
				center: active.center,
			});
		}
		if action.is_none() && drag.is_none() && view.nudge_enabled {
			action = input::nudge(ui, &response);
		}
		let background = match style.background {
			Some(Background::Color { value }) => parse_solid_background_color(value.as_str())
				.map(canvas_color)
				.unwrap_or(CANVAS_BACKGROUND),
			_ => CANVAS_BACKGROUND,
		};
		let painter = ui.painter_at(rect);
		painter.rect_filled(rect, ITEM_CORNER, background);
		painter.rect_stroke(
			rect,
			ITEM_CORNER,
			if response.has_focus() {
				ui.visuals().selection.stroke
			} else {
				Stroke::new(1.0, CANVAS_EDGE)
			},
			StrokeKind::Inside,
		);
		for item in layout
			.items()
			.iter()
			.filter(|item| !item.hidden || view.show_hidden)
		{
			let center = drag
				.as_ref()
				.filter(|active| active.path == *item.destination)
				.map_or(item.center, |active| active.center);
			let Some(center) = project(viewport, center) else {
				continue;
			};
			let half = (style.icon_size.width() * view.scale / 2.0).min(MAX_CANVAS_POINTS) as f32;
			let icon = Rect::from_center_size(center, vec2(half * 2.0, half * 2.0));
			if !rect.intersects(icon.expand(half)) {
				continue;
			}
			let highlighted = view.selected.contains(item.destination);
			painter.rect_filled(
				icon,
				ITEM_CORNER,
				if highlighted {
					ITEM_SELECTED
				} else {
					ITEM_SURFACE
				},
			);
			painter.rect_stroke(
				icon,
				ITEM_CORNER,
				Stroke::new(1.0, ITEM_EDGE),
				StrokeKind::Inside,
			);
			let visible_icon = icon.intersect(rect);
			if drag.is_none()
				&& visible_icon.is_positive()
				&& view
					.lint_warnings
					.is_some_and(|warnings| warnings.contains(item.destination.as_str()))
			{
				painter.rect_stroke(
					icon,
					ITEM_CORNER,
					Stroke::new(LINT_BORDER_WIDTH, LINT_WARNING),
					StrokeKind::Inside,
				);
				let badge = pos2(
					visible_icon.right() - LINT_MARK_RADIUS,
					visible_icon.top() + LINT_MARK_RADIUS,
				);
				painter.circle_filled(badge, LINT_MARK_RADIUS, LINT_WARNING);
				painter.text(
					badge,
					Align2::CENTER_CENTER,
					LINT_MARK_TEXT,
					FontId::proportional(LINT_MARK_FONT_SIZE),
					Color32::WHITE,
				);
			}
			let mark_size = half / 2.3;
			let mark = Rect::from_center_size(center, vec2(mark_size, mark_size * 1.2));
			painter.rect_stroke(
				mark,
				2,
				Stroke::new(
					1.5,
					if highlighted {
						Color32::WHITE
					} else {
						ITEM_FOREGROUND
					},
				),
				StrokeKind::Inside,
			);
			let label = item.display_name();
			let (anchor, align) = match style.label_position {
				LabelPosition::Right => (
					pos2(icon.right() + LABEL_MARGIN, center.y),
					Align2::LEFT_CENTER,
				),
				_ => (
					pos2(center.x, icon.bottom() + LABEL_MARGIN),
					Align2::CENTER_TOP,
				),
			};
			let font_size = (style.text_size * view.scale).min(MAX_LABEL_POINTS) as f32;
			let galley = painter.layout(
				label.to_owned(),
				FontId::proportional(font_size),
				ITEM_FOREGROUND,
				canvas_size.x,
			);
			let label_rect = align.anchor_size(anchor, galley.size());
			painter.rect_filled(label_rect.expand2(vec2(4.0, 2.0)), 3, CANVAS_BACKGROUND);
			painter.galley(label_rect.min, galley, ITEM_FOREGROUND);
		}
		ui.add_space(FIT_PADDING);
	});
	(action, drag)
}

fn canvas_point(point: Pos2) -> Option<CanvasPoint> {
	CanvasPoint::new(f64::from(point.x), f64::from(point.y)).ok()
}

fn project(viewport: Viewport, center: FolderPoint) -> Option<Pos2> {
	let point = viewport.to_canvas(center).ok()?;
	let position = pos2(point.x() as f32, point.y() as f32);
	(position.x.is_finite() && position.y.is_finite()).then_some(position)
}

fn canvas_color((red, green, blue): (f64, f64, f64)) -> Color32 {
	let channel = |normalized: f64| (normalized * f64::from(u8::MAX)).round() as u8;
	Color32::from_rgb(channel(red), channel(green), channel(blue))
}

#[cfg(test)]
mod tests {
	use dmgez_config::Project;
	use dmgez_preview::ItemInput;
	use eframe::egui::{Context, Event, Key, Modifiers, PointerButton, RawInput, Shape};

	use super::*;
	use crate::project::arrangement::Arrangement;

	const PLAN: &str = "schema_version=1\n[image]\nname='Demo'\noutput='demo.dmg'\n[[contents]]\nsource='absent.txt'\ndestination='/Demo.txt'\nposition=[70,50]\n[finder]\nwindow_size=[320,190]\n[finder.icon_view]\nicon_size=32\n";

	#[test]
	fn paints_the_shared_solid_background_color() {
		const SOURCE: &str = r#"
schema_version = 1
[image]
name = 'Color'
output = 'color.dmg'
[finder]
background = { kind = 'color', value = 'hsl(180deg, 100%, 50%)' }
"#;
		let project = Project::parse(SOURCE).unwrap();
		let folder = "/".parse().unwrap();
		let layout = ProjectFolder::from_project(&project, &folder, &[]).unwrap();
		let mut frame = Context::default().run_ui(
			RawInput {
				screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(720.0, 550.0))),
				..Default::default()
			},
			|ui| {
				let view = View {
					selected: &BTreeSet::new(),
					nudge_enabled: false,
					show_hidden: false,
					snap: false,
					scale: 1.0,
					drag: None,
					lint_warnings: None,
				};
				show(ui, &layout, view, |_| true);
			},
		);
		frame.textures_delta.clear();
		assert!(frame.shapes.iter().any(|shape| {
			matches!(&shape.shape, Shape::Rect(rect) if rect.fill == Color32::from_rgb(0, 255, 255))
		}));
	}

	#[test]
	fn pointer_selects_the_icon_with_or_without_a_lint_marker() {
		let ctx = Context::default();
		let project = Project::parse(PLAN).unwrap();
		let folder = "/".parse().unwrap();
		let children = [ItemInput::from(&project.contents[0])];
		let layout = ProjectFolder::from_project(&project, &folder, &children).unwrap();
		let warnings = BTreeSet::from(["/Demo.txt".to_owned()]);
		let mut action = None;
		let mut frame = |events, show_lint: bool| {
			let mut output = ctx.run_ui(
				RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(720.0, 550.0))),
					events,
					..Default::default()
				},
				|ui| {
					let view = View {
						selected: &BTreeSet::new(),
						nudge_enabled: false,
						show_hidden: false,
						snap: false,
						scale: 1.0,
						drag: None,
						lint_warnings: show_lint.then_some(&warnings),
					};
					if let (Some(selected), _) = show(ui, &layout, view, |_| true) {
						action = Some(selected);
					}
				},
			);
			output.textures_delta.clear();
			output
		};
		let first = frame(Vec::new(), false);
		let icon = first
			.shapes
			.iter()
			.find_map(|shape| match &shape.shape {
				Shape::Rect(rect) if rect.fill == ITEM_SURFACE => Some(rect.rect.center()),
				_ => None,
			})
			.expect("the fixture icon was painted");
		let is_badge = |shape: &eframe::egui::epaint::ClippedShape| matches!(&shape.shape, Shape::Circle(circle) if circle.fill == LINT_WARNING);
		assert!(!first.shapes.iter().any(is_badge));
		let marked = frame(Vec::new(), true);
		assert!(marked.shapes.iter().any(is_badge));
		let click = |pressed| Event::PointerButton {
			pos: icon,
			button: PointerButton::Primary,
			pressed,
			modifiers: Modifiers::NONE,
		};
		let _ = frame(vec![Event::PointerMoved(icon), click(true)], true);
		let _ = frame(vec![click(false)], true);
		assert!(
			matches!(action, Some(Action::Select { path, toggle: false }) if path.as_str() == "/Demo.txt")
		);
	}

	#[test]
	fn focused_canvas_toggles_and_turns_arrow_events_into_one_nudge() {
		let context = Context::default();
		let project = Project::parse(PLAN).unwrap();
		let folder = "/".parse().unwrap();
		let children = [ItemInput::from(&project.contents[0])];
		let layout = ProjectFolder::from_project(&project, &folder, &children).unwrap();
		let mut actions = Vec::new();
		let mut frame = |events: Vec<Event>, modifiers| {
			let mut output = context.run_ui(
				RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(720.0, 550.0))),
					events: std::iter::once(Event::ModifiersChanged(modifiers))
						.chain(events)
						.collect(),
					..Default::default()
				},
				|ui| {
					let view = View {
						selected: &BTreeSet::new(),
						nudge_enabled: true,
						show_hidden: false,
						snap: true,
						scale: 2.0,
						drag: None,
						lint_warnings: None,
					};
					if let (Some(action), _) = show(ui, &layout, view, |_| true) {
						actions.push(action);
					}
				},
			);
			output.textures_delta.clear();
			output
		};
		let first = frame(Vec::new(), Modifiers::NONE);
		let icon = first
			.shapes
			.iter()
			.find_map(|shape| match &shape.shape {
				Shape::Rect(rect) if rect.fill == ITEM_SURFACE => Some(rect.rect.center()),
				_ => None,
			})
			.unwrap();
		let click = |pressed| Event::PointerButton {
			pos: icon,
			button: PointerButton::Primary,
			pressed,
			modifiers: Modifiers::SHIFT,
		};
		let key = |key, modifiers, pressed| Event::Key {
			key,
			physical_key: None,
			pressed,
			repeat: false,
			modifiers,
		};
		let _ = frame(
			vec![key(Key::ArrowLeft, Modifiers::NONE, true)],
			Modifiers::NONE,
		);
		let _ = frame(
			vec![Event::PointerMoved(icon), click(true)],
			Modifiers::SHIFT,
		);
		let _ = frame(vec![click(false)], Modifiers::SHIFT);
		let _ = frame(Vec::new(), Modifiers::NONE);
		let _ = frame(
			vec![
				key(Key::ArrowRight, Modifiers::NONE, true),
				key(Key::ArrowUp, Modifiers::SHIFT, true),
			],
			Modifiers::SHIFT,
		);
		let _ = frame(
			vec![key(Key::ArrowRight, Modifiers::NONE, true)],
			Modifiers::NONE,
		);

		let [
			Action::Select { path, toggle: true },
			Action::Arrange(Arrangement::Nudge([dx, dy])),
		] = actions.as_slice()
		else {
			panic!(
				"expected one toggle and one nudge, received {} actions",
				actions.len()
			);
		};
		assert_eq!(path.as_str(), "/Demo.txt");
		assert_eq!([*dx, *dy], [1.0, -10.0]);
	}
}
