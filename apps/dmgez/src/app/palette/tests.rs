use std::time::Duration;

use eframe::egui::{Event, Pos2, RawInput, Rect, Vec2};

use super::super::{Pending, open_outcome};
use super::*;

const FIXTURE: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const PROJECT: &str = "schema_version=1\n[image]\nname='Palette'\noutput='build.dmg'\n";

fn key(key: Key, modifiers: Modifiers) -> Event {
	Event::Key {
		key,
		physical_key: Some(key),
		pressed: true,
		repeat: false,
		modifiers,
	}
}

fn frame(app: &mut App, ctx: &Context, events: Vec<Event>) {
	let input = RawInput {
		screen_rect: Some(Rect::from_min_size(
			Pos2::ZERO,
			Vec2::from(crate::MINIMUM_SIZE),
		)),
		events,
		..RawInput::default()
	};
	ctx.run_ui(input, |_| {
		app.shortcuts(ctx);
		app.show_palette(ctx);
	})
	.drop_without_applying_deltas();
}

fn show(app: &mut App, ctx: &Context) {
	frame(
		app,
		ctx,
		vec![key(SHORTCUT.logical_key, SHORTCUT.modifiers)],
	);
	frame(app, ctx, vec![]);
	assert!(ctx.memory(|memory| memory.has_focus(Id::unique(SEARCH_ID))));
}

#[test]
fn shortcut_search_and_enter_route_to_the_existing_image_verifier() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("fixture.dmg");
	std::fs::write(&path, FIXTURE).unwrap();
	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	assert_eq!(app.available_commands(), [Command::Open]);
	app.finish(&ctx, open_outcome(path));
	assert!(!app.available_commands().contains(&Command::SaveCopy));
	assert!(
		!app.available_commands()
			.contains(&Command::ExportVerification)
	);
	show(&mut app, &ctx);
	assert!(app.palette.is_some());
	frame(&mut app, &ctx, vec![Event::Text("no such command".into())]);
	frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
	assert!(app.palette.is_some());
	assert!(app.worker.is_none());
	frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
	assert!(app.palette.is_none());
	show(&mut app, &ctx);
	frame(&mut app, &ctx, vec![Event::Text("verify container".into())]);
	frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
	assert!(app.palette.is_none());
	let worker = app.worker.take().expect("verification worker");
	app.finish(&ctx, worker.receiver.recv_timeout(WORKER_TIMEOUT).unwrap());
	let Some(Opened {
		document: Document::Image(image),
		..
	}) = &app.opened
	else {
		panic!("expected the same open image");
	};
	assert!(image.verification.as_ref().unwrap().passed());
}

#[test]
fn opening_from_the_palette_preserves_the_unsaved_changes_prompt() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("fixture.toml");
	std::fs::write(&path, PROJECT).unwrap();
	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	app.finish(&ctx, open_outcome(path));
	let Some(Opened {
		document: Document::Project(project),
		..
	}) = &mut app.opened
	else {
		panic!("expected the open project");
	};
	project.apply_edits(&[dmgez_config::project_edit::ProjectEdit::SetImageName(
		"Edited".into(),
	)]);
	assert!(app.is_dirty());
	assert!(!app.available_commands().contains(&Command::Verify));
	show(&mut app, &ctx);
	frame(&mut app, &ctx, vec![Event::Text("open".into())]);
	assert_eq!(app.palette.as_ref().unwrap().query, "open");
	frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
	assert!(matches!(app.confirm, Some(Pending::Open)));
	assert!(app.palette.is_none());
	assert!(app.worker.is_none());
	assert!(app.is_dirty());
}
