use std::{fs, sync::mpsc, time::Duration};

use super::*;
use crate::{app::open_outcome, inspector::Action};

const UDZO: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");
const RAW: &[u8] = include_bytes!("../../../../../tests/fixtures/expected/tiny.raw");
const TIMEOUT: Duration = Duration::from_secs(5);

fn finish_worker(app: &mut App, ctx: &Context) {
	let worker = app.worker.take().expect("export action started a worker");
	let result = worker
		.receiver
		.recv_timeout(TIMEOUT)
		.expect("worker completed");
	app.finish(ctx, result);
}

fn opened_image(app: &App) -> &ImageTab {
	let Document::Image(image) = &app.opened.as_ref().unwrap().document else {
		panic!("expected image tab");
	};
	image
}

fn open_fixture(ctx: &Context, scratch: &Path) -> (App, PathBuf) {
	let path = scratch.join("fixture.dmg");
	fs::write(&path, UDZO).unwrap();
	let mut app = App::new(ctx, None);
	app.finish(ctx, open_outcome(path.clone()));
	(app, path)
}

fn export_to(app: &mut App, ctx: &Context, kind: Kind, path: &Path) {
	let destination = path.to_owned();
	app.start_image_export_with(ctx, kind, move |_, _| Some(destination));
	finish_worker(app, ctx);
}

#[test]
fn inspector_exports_the_selected_run_and_both_versioned_reports() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, source) = open_fixture(&ctx, scratch.path());
	app.image_action(&ctx, Action::LocateRun(0));
	finish_worker(&mut app, &ctx);
	let logical = opened_image(&app).report.run_page.as_ref().unwrap().runs[0].logical;
	let from = usize::try_from(logical.offset_bytes).unwrap();
	let len = usize::try_from(logical.length_bytes).unwrap();

	let inspection = scratch.path().join("inspection.json");
	export_to(&mut app, &ctx, Kind::Inspection, &inspection);
	let report: serde_json::Value =
		serde_json::from_slice(&fs::read(&inspection).unwrap()).unwrap();
	assert_eq!(report["schema_version"], dmgez_inspector::SCHEMA_VERSION);
	assert_eq!(report["operation"], "inspect");
	assert_eq!(report["run_page"]["offset"], "0");
	assert_eq!(
		opened_image(&app).exported.as_deref(),
		Some(inspection.as_path())
	);

	app.image_action(&ctx, Action::Verify);
	finish_worker(&mut app, &ctx);
	let verification = scratch.path().join("verification.json");
	export_to(&mut app, &ctx, Kind::Verification, &verification);
	let report: serde_json::Value =
		serde_json::from_slice(&fs::read(&verification).unwrap()).unwrap();
	assert_eq!(report["schema_version"], dmgez_inspector::SCHEMA_VERSION);
	assert_eq!(report["operation"], "verify");
	assert_eq!(report["outcome"], "passed");

	let raw = scratch.path().join("run.raw");
	export_to(&mut app, &ctx, Kind::Run(0), &raw);
	assert_eq!(fs::read(&raw).unwrap(), RAW[from..from + len]);
	assert_eq!(opened_image(&app).exported.as_deref(), Some(raw.as_path()));
	assert!(app.error.is_none());
	assert_eq!(fs::read(source).unwrap(), UDZO);
}

#[test]
fn export_keeps_existing_paths_and_refuses_reports_from_a_stale_source() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, source) = open_fixture(&ctx, scratch.path());
	let existing = scratch.path().join("existing.json");
	fs::write(&existing, b"occupied").unwrap();
	export_to(&mut app, &ctx, Kind::Inspection, &existing);
	assert_eq!(fs::read(existing).unwrap(), b"occupied");
	assert!(app.error.as_deref().unwrap().contains("without replacing"));

	app.image_action(&ctx, Action::LocateRun(0));
	finish_worker(&mut app, &ctx);
	fs::write(source, []).unwrap();
	let run = scratch.path().join("stale-run.raw");
	export_to(&mut app, &ctx, Kind::Run(0), &run);
	assert!(!run.exists());
	assert!(app.error.as_deref().unwrap().contains("source changed"));
	assert!(opened_image(&app).run_navigation.selected.is_none());
	let report = scratch.path().join("stale.json");
	export_to(&mut app, &ctx, Kind::Inspection, &report);
	assert!(!report.exists());
	assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 2);
}

#[test]
fn cancelling_before_export_stops_publication_and_late_cancellation_keeps_the_result() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, _) = open_fixture(&ctx, scratch.path());
	let stopped = scratch.path().join("stopped.json");
	let (send, receive) = mpsc::sync_channel(1);
	app.start_image_export_with(&ctx, Kind::Inspection, move |_, _| {
		receive.recv_timeout(TIMEOUT).ok()
	});
	app.worker
		.as_ref()
		.unwrap()
		.cancellation
		.as_ref()
		.unwrap()
		.cancel();
	send.send(stopped.clone()).unwrap();
	finish_worker(&mut app, &ctx);
	assert!(!stopped.exists());
	assert!(app.error.is_none());

	let saved = scratch.path().join("saved.json");
	let destination = saved.clone();
	app.start_image_export_with(&ctx, Kind::Inspection, move |_, _| Some(destination));
	let worker = app.worker.as_mut().unwrap();
	let completed = worker.receiver.recv_timeout(TIMEOUT).unwrap();
	assert!(saved.is_file());
	worker.cancellation.as_ref().unwrap().cancel();
	// Deliver the real completion to poll after requesting cancellation.
	let (send, receive) = mpsc::sync_channel(1);
	send.send(completed).unwrap();
	worker.receiver = receive;
	app.poll(&ctx);
	assert_eq!(
		opened_image(&app).exported.as_deref(),
		Some(saved.as_path())
	);
}
