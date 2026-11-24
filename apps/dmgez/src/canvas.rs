use dmgez_config::{Background, LabelPosition, VirtualPath};
use dmgez_preview::{
	CanvasPoint, Drag, FolderPoint, ItemVisibility, PositionSource, ProjectFolder, Viewport,
};
use eframe::egui::{
	Align2, Color32, FontId, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::project::{Action, DragEdit};

const MAX_CANVAS_POINTS: f64 = 32_768.0;
const CANVAS_EDGE: Color32 = Color32::from_rgb(184, 199, 199);
const CANVAS_BACKGROUND: Color32 = Color32::from_rgb(245, 249, 248);
const ITEM_SELECTED: Color32 = Color32::from_rgb(0, 111, 107);
const ITEM_SURFACE: Color32 = Color32::from_rgb(215, 234, 229);
const ITEM_EDGE: Color32 = Color32::from_rgb(103, 132, 129);
const ITEM_FOREGROUND: Color32 = Color32::from_rgb(34, 56, 60);
const ITEM_CORNER: u8 = 8;
const LABEL_MARGIN: f32 = 5.0;
const MAX_LABEL_POINTS: f64 = 36.0;
const FIT_PADDING: f32 = 26.0;
const HEX_RADIX: u32 = 16;
const HEX_SHORT_SCALE: u8 = 17;

pub(crate) struct View<'a> {
	pub selected: Option<&'a VirtualPath>,
	pub show_hidden: bool,
	pub snap: bool,
	pub scale: f64,
	pub drag: Option<&'a DragEdit>,
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
	ScrollArea::both().show(ui, |ui| {
		ui.add_space(FIT_PADDING);
		let canvas_size = vec2(width as f32, height as f32);
		let (rect, response) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
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
			&& let Some(item) = response.interact_pointer_pos().and_then(hit)
		{
			action = Some(Action::Select(item.destination.clone()));
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
			action = Some(Action::Select(item.destination.clone()));
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
		let background = match style.background {
			Some(Background::Color { value }) => {
				parse_color(value.as_str()).unwrap_or(CANVAS_BACKGROUND)
			}
			_ => CANVAS_BACKGROUND,
		};
		let painter = ui.painter_at(rect);
		painter.rect_filled(rect, ITEM_CORNER, background);
		painter.rect_stroke(
			rect,
			ITEM_CORNER,
			Stroke::new(1.0, CANVAS_EDGE),
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
			let highlighted = view.selected == Some(item.destination);
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
			let label = if item.hide_extension {
				item.name.rsplit_once('.').map_or(item.name, |(stem, _)| {
					if stem.is_empty() { item.name } else { stem }
				})
			} else {
				item.name
			};
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

fn parse_color(text: &str) -> Option<Color32> {
	let data = text.strip_prefix('#')?.as_bytes();
	let digit = |value| u8::try_from(char::from(value).to_digit(HEX_RADIX)?).ok();
	let pair = |a, b| Some(digit(a)? * (HEX_RADIX as u8) + digit(b)?);
	let [r, g, b] = match *data {
		[r, g, b] => [
			digit(r)? * HEX_SHORT_SCALE,
			digit(g)? * HEX_SHORT_SCALE,
			digit(b)? * HEX_SHORT_SCALE,
		],
		[r1, r2, g1, g2, b1, b2] => [pair(r1, r2)?, pair(g1, g2)?, pair(b1, b2)?],
		_ => return None,
	};
	Some(Color32::from_rgb(r, g, b))
}

#[cfg(test)]
mod tests {
	use dmgez_config::Project;
	use dmgez_preview::ItemInput;
	use eframe::egui::{Context, Event, Modifiers, PointerButton, RawInput, Shape};

	use super::*;

	#[test]
	fn pointer_selects_the_icon_at_its_projected_center() {
		const SOURCE: &str = "schema_version=1\n[image]\nname='Demo'\noutput='demo.dmg'\n[[contents]]\nsource='absent.txt'\ndestination='/Demo.txt'\nposition=[70,50]\n[finder]\nwindow_size=[320,190]\n[finder.icon_view]\nicon_size=32\n";
		let ctx = Context::default();
		let project = Project::parse(SOURCE).unwrap();
		let folder = "/".parse().unwrap();
		let children = [ItemInput::from(&project.contents[0])];
		let layout = ProjectFolder::from_project(&project, &folder, &children).unwrap();
		let mut action = None;
		let mut frame = |events| {
			let mut output = ctx.run_ui(
				RawInput {
					screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(720.0, 550.0))),
					events,
					..Default::default()
				},
				|ui| {
					let view = View {
						selected: None,
						show_hidden: false,
						snap: false,
						scale: 1.0,
						drag: None,
					};
					if let (Some(selected), _) = show(ui, &layout, view, |_| true) {
						action = Some(selected);
					}
				},
			);
			output.textures_delta.clear();
			output
		};
		let first = frame(Vec::new());
		let icon = first
			.shapes
			.iter()
			.find_map(|shape| match &shape.shape {
				Shape::Rect(rect) if rect.fill == ITEM_SURFACE => Some(rect.rect.center()),
				_ => None,
			})
			.expect("the fixture icon was painted");
		let click = |pressed| Event::PointerButton {
			pos: icon,
			button: PointerButton::Primary,
			pressed,
			modifiers: Modifiers::NONE,
		};
		let _ = frame(vec![Event::PointerMoved(icon), click(true)]);
		let _ = frame(vec![click(false)]);
		assert!(matches!(action, Some(Action::Select(path)) if path.as_str() == "/Demo.txt"));
	}
}
