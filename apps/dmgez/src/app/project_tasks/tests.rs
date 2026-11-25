use std::{fs, sync::mpsc, time::Duration};

use dmgez_config::project_edit::{ContentDraft, ProjectEdit};

use super::*;
use crate::app::Worker;

const PROJECT_NAME: &str = "input.toml";
const MANIFEST: &str = "# Retain this on disk\nschema_version=1\n[image]\nname='Snapshot'\noutput='saved.dmg'\n[metadata]\npolicy='report'\n";
const WORKER_TIMEOUT: Duration = Duration::from_secs(15);
const EDITED_OUTPUT: &str = "unsaved.dmg";
const DESTINATION: &str = "explicit-output";
const PAYLOAD_NAME: &str = "payload.txt";
const PAYLOAD: &[u8] = b"content known only to the unsaved project snapshot";

fn editor(ctx: &Context, directory: &Path) -> App {
	let project_path = directory.join(PROJECT_NAME);
	fs::write(&project_path, MANIFEST).unwrap();
	fs::write(directory.join(PAYLOAD_NAME), PAYLOAD).unwrap();
	let mut app = App::new(ctx, None);
	app.opened = Some(document::open(project_path).unwrap());
	let Some(Opened {
		document: Document::Project(project),
		..
	}) = &mut app.opened
	else {
		panic!("expected an opened project");
	};
	project.apply_edits(&[
		ProjectEdit::Add(ContentDraft::Copy {
			source: PAYLOAD_NAME.into(),
			destination: None,
		}),
		ProjectEdit::SetOutputPath(EDITED_OUTPUT.into()),
	]);
	assert!(project.is_dirty());
	app
}

fn deliver(app: &mut App, ctx: &Context, cancel_after_completion: bool) {
	let worker = app.worker.take().expect("expected a build worker");
	let result = worker.receiver.recv_timeout(WORKER_TIMEOUT).unwrap();
	let cancellation = worker.cancellation.clone();
	let (send, receiver) = mpsc::sync_channel(1);
	send.send(result).unwrap();
	app.worker = Some(Worker {
		receiver,
		cancellation,
	});
	if cancel_after_completion {
		app.worker
			.as_ref()
			.unwrap()
			.cancellation
			.as_ref()
			.unwrap()
			.cancel();
	}
	app.poll(ctx);
	assert!(app.worker.is_none());
	assert!(app.build.progress.is_none());
}

#[test]
fn dry_run_uses_the_unsaved_snapshot_and_original_project_base() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let directory = scratch.path().canonicalize().unwrap();
	let mut app = editor(&ctx, &directory);
	app.start_project_build(&ctx, BuildMode::Plan, |_| {
		panic!("planning does not pick a destination")
	});
	deliver(&mut app, &ctx, false);
	let completed = app.build.completed.unwrap();
	assert!(completed.unsaved);
	let Some(BuildOutcome::Planned(plan)) = completed.outcome.unwrap() else {
		panic!("expected a build plan");
	};
	assert_eq!(plan.source_payload_bytes, PAYLOAD.len() as u64);
	assert_eq!(plan.output, Some(directory.join(EDITED_OUTPUT)));
	assert!(!directory.join(EDITED_OUTPUT).exists());
	assert_eq!(
		fs::read_to_string(directory.join(PROJECT_NAME)).unwrap(),
		MANIFEST
	);
}

#[test]
fn a_cancelled_build_does_not_publish_its_selected_destination() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let directory = scratch.path().canonicalize().unwrap();
	let destination = directory.join(DESTINATION);
	let mut app = editor(&ctx, &directory);
	let (picker_started, entered_picker) = mpsc::sync_channel(1);
	let (resume, wait) = mpsc::sync_channel(1);
	app.start_project_build(&ctx, BuildMode::Publish, move |_| {
		picker_started.send(()).unwrap();
		Some(wait.recv_timeout(WORKER_TIMEOUT).unwrap())
	});
	entered_picker.recv_timeout(WORKER_TIMEOUT).unwrap();
	assert!(app.cancel_build_for_close(&ctx));
	resume.send(destination.clone()).unwrap();
	deliver(&mut app, &ctx, false);
	assert!(app.build.completed.unwrap().outcome.unwrap().is_none());
	assert!(!destination.exists());
}

#[test]
fn publication_remains_visible_when_cancellation_arrives_before_delivery() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let directory = scratch.path().canonicalize().unwrap();
	let destination = directory.join(DESTINATION);
	let selected = destination.clone();
	let mut app = editor(&ctx, &directory);
	app.start_project_build(&ctx, BuildMode::Publish, move |_| Some(selected));
	deliver(&mut app, &ctx, true);
	let completed = app.build.completed.unwrap();
	assert!(completed.unsaved);
	let Some(BuildOutcome::Published { output, .. }) = completed.outcome.unwrap() else {
		panic!("expected a completed publication despite late cancellation");
	};
	assert_eq!(output, Some(destination.clone()));
	assert_eq!(
		fs::read_to_string(directory.join(PROJECT_NAME)).unwrap(),
		MANIFEST
	);
	let Completion::Opened(Opened {
		document: Document::Image(image),
		..
	}) = open_built_outcome(destination)
	else {
		panic!("expected an inspectable output despite its missing file suffix");
	};
	assert!(image.snapshot.verify(&Cancellation::default()).passed());
}
