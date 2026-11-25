use std::{fs, time::Duration};

use dmgez_config::{Payload, project_edit::ProjectDocument};

use super::*;
use crate::project::ProjectTab;

const MANIFEST: &str =
	"# original comment\nschema_version=1\n[image]\nname='Add files'\noutput='build.dmg'\n";
const PROJECT_NAME: &str = "input.toml";
const COPY_NAME: &str = "saved.toml";
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

fn project(app: &mut App) -> &mut ProjectTab {
	let Some(Opened {
		document: Document::Project(project),
		..
	}) = &mut app.opened
	else {
		panic!("expected a project document");
	};
	project
}

fn receive(app: &mut App, ctx: &Context) {
	let receiver = app.worker.take().expect("expected one active worker");
	let reply = receiver
		.receiver
		.recv_timeout(WORKER_TIMEOUT)
		.expect("expected worker response");
	app.finish(ctx, reply);
}

#[test]
fn one_worker_selection_adds_multiple_files_with_grouped_history_and_stable_paths() {
	let scratch = tempfile::tempdir().unwrap();
	let base = scratch.path().canonicalize().unwrap();
	let project_dir = base.join("project");
	fs::create_dir(&project_dir).unwrap();
	let path = project_dir.join(PROJECT_NAME);
	let internal = project_dir.join("inside.txt");
	let external = base.join("outside.txt");
	fs::write(&path, MANIFEST).unwrap();
	fs::write(&internal, b"inside").unwrap();
	fs::write(&external, b"outside").unwrap();

	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	app.finish(&ctx, open_outcome(path.clone()));
	let expected_dir = project_dir.clone();
	let selected = vec![internal.clone(), external.clone()];
	app.add_files(&ctx, move |directory| {
		assert_eq!(directory, expected_dir);
		Some(selected)
	});
	app.add_files(&ctx, |_| panic!("a busy worker must reject another picker"));
	assert!(project(&mut app).document.project().contents.is_empty());
	receive(&mut app, &ctx);

	let project = project(&mut app);
	assert!(project.is_dirty());
	assert_eq!(project.document.project().contents.len(), 2);
	project.undo();
	assert!(!project.is_dirty());
	assert!(project.document.project().contents.is_empty());
	project.redo();
	assert_eq!(project.document.project().contents.len(), 2);

	let (copy, _) = document::save_copy(&path, COPY_NAME, project.document.source()).unwrap();
	let reopened = ProjectDocument::open(&fs::read_to_string(&copy).unwrap()).unwrap();
	assert!(reopened.source().starts_with("# original comment"));
	for (content, expected) in reopened.project().contents.iter().zip([internal, external]) {
		let Payload::Copy(content) = &content.payload else {
			panic!("expected a copy")
		};
		assert_eq!(content.source.resolve(copy.parent().unwrap()), expected);
	}
	assert_eq!(fs::read_to_string(path).unwrap(), MANIFEST);
}

#[test]
fn cancelled_or_conflicting_selection_keeps_the_original_revision() {
	let scratch = tempfile::tempdir().unwrap();
	let base = scratch.path().canonicalize().unwrap();
	let path = base.join(PROJECT_NAME);
	let first_dir = base.join("first");
	let second_dir = base.join("second");
	fs::create_dir(&first_dir).unwrap();
	fs::create_dir(&second_dir).unwrap();
	let first = first_dir.join("same.txt");
	let second = second_dir.join("same.txt");
	fs::write(&path, MANIFEST).unwrap();
	fs::write(&first, b"first").unwrap();
	fs::write(&second, b"second").unwrap();

	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	app.finish(&ctx, open_outcome(path));
	let revision = project(&mut app).document.revision();
	app.add_files(&ctx, |_| None);
	receive(&mut app, &ctx);
	assert_eq!(project(&mut app).document.revision(), revision);

	app.add_files(&ctx, |_| Some(vec![first, second]));
	receive(&mut app, &ctx);
	let project = project(&mut app);
	assert_eq!(project.document.revision(), revision);
	assert!(project.document.project().contents.is_empty());
	assert_eq!(project.document.source(), MANIFEST);
}
