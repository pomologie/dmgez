use std::{fs, sync::Arc, time::Duration};

use dmgez_config::{Payload, project_edit::ProjectDocument};
use eframe::egui::{DroppedFile, DroppedFileHandle, Event, Pos2, RawInput, Rect, accesskit};

use super::*;
use crate::project::ProjectTab;

const MANIFEST: &str =
	"# original comment\nschema_version=1\n[image]\nname='Add files'\noutput='build.dmg'\n";
const PROJECT_NAME: &str = "input.toml";
const COPY_NAME: &str = "saved.toml";
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const DOCS: &str = "/Docs";

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

#[test]
fn a_native_drop_adds_files_and_directories_to_the_active_folder_in_one_edit() {
	let scratch = tempfile::tempdir().unwrap();
	let base = scratch.path().canonicalize().unwrap();
	let project_dir = base.join("project");
	let path = project_dir.join(PROJECT_NAME);
	let bundle = project_dir.join("Example.app");
	let outside = base.join("notes.txt");
	let manifest = format!("{MANIFEST}\n[[contents]]\ntype='directory'\ndestination='{DOCS}'\n");
	fs::create_dir_all(&bundle).unwrap();
	fs::write(bundle.join("payload"), b"bundle payload").unwrap();
	fs::write(&outside, b"external file").unwrap();
	fs::write(&path, &manifest).unwrap();

	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	app.finish(&ctx, open_outcome(path.clone()));
	enter_docs(&ctx, &mut app);
	let project_before = project(&mut app);
	project_before.apply_edits(&[ProjectEdit::SetImageName("Unsaved".into())]);
	let revision = project_before.document.revision();
	let source = project_before.document.source().to_owned();

	drop_paths(&ctx, &mut app, [bundle.clone(), outside.clone()]);
	assert_eq!(project(&mut app).document.revision(), revision);
	receive(&mut app, &ctx);

	let project = project(&mut app);
	assert_eq!(project.folder().as_str(), DOCS);
	let [_, bundle_copy, file_copy] = project.document.project().contents.as_slice() else {
		panic!("expected one directory declaration and two copy declarations");
	};
	assert_eq!(bundle_copy.destination.as_str(), "/Docs/Example.app");
	assert_eq!(file_copy.destination.as_str(), "/Docs/notes.txt");
	let (Payload::Copy(bundle_copy), Payload::Copy(file_copy)) =
		(&bundle_copy.payload, &file_copy.payload)
	else {
		panic!("expected copy declarations for both drop entries");
	};
	assert!(bundle_copy.recursive);
	assert_eq!(bundle_copy.source.as_path(), Path::new("Example.app"));
	assert_eq!(file_copy.source.as_path(), outside);
	project.undo();
	assert_eq!(project.document.source(), source);
	assert_eq!(project.document.revision(), revision);
	assert!(project.is_dirty());
	project.redo();
	assert_eq!(project.document.project().contents.len(), 3);
	assert_eq!(fs::read_to_string(path).unwrap(), manifest);
	assert_eq!(fs::read(bundle.join("payload")).unwrap(), b"bundle payload");
}

#[test]
fn blocked_or_invalid_drops_leave_the_open_project_revision_unchanged() {
	let scratch = tempfile::tempdir().unwrap();
	let base = scratch.path().canonicalize().unwrap();
	let path = base.join(PROJECT_NAME);
	let input = base.join("input.txt");
	fs::write(&path, MANIFEST).unwrap();
	fs::write(&input, b"content").unwrap();
	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	drop_paths(&ctx, &mut app, [input.clone()]);
	assert!(app.opened.is_none());
	assert!(app.worker.is_none());
	app.finish(&ctx, open_outcome(path));
	let revision = project(&mut app).document.revision();

	app.confirm = Some(Pending::Open);
	drop_paths(&ctx, &mut app, [input.clone()]);
	assert!(app.worker.is_none());
	app.confirm = None;
	drop_paths(&ctx, &mut app, []);
	assert!(app.worker.is_none());

	app.add_files(&ctx, |_| None);
	drop_paths(&ctx, &mut app, [input.clone()]);
	receive(&mut app, &ctx);
	assert_eq!(project(&mut app).document.revision(), revision);

	drop_paths(&ctx, &mut app, [input, PathBuf::from("relative.txt")]);
	receive(&mut app, &ctx);
	assert!(
		app.error
			.as_deref()
			.unwrap()
			.contains("absolute file location")
	);
	assert_eq!(project(&mut app).document.revision(), revision);
	assert!(!app.is_dirty());
}

fn enter_docs(ctx: &Context, app: &mut App) {
	ctx.enable_accesskit();
	let mut output = ctx.run_ui(raw_input(), |ui| project(app).ui(ui));
	let nodes = output
		.platform_output
		.accesskit_update
		.take()
		.unwrap()
		.nodes;
	output.drop_without_applying_deltas();
	let (id, entry) = nodes
		.iter()
		.find(|(_, node)| node.label() == Some("Open"))
		.expect("expected an accessible folder-opening control");
	assert!(entry.supports_action(accesskit::Action::Click));
	ctx.run_ui(
		RawInput {
			events: vec![Event::AccessKitActionRequest(accesskit::ActionRequest {
				action: accesskit::Action::Click,
				target_tree: accesskit::TreeId::ROOT,
				target_node: *id,
				data: None,
			})],
			..raw_input()
		},
		|ui| project(app).ui(ui),
	)
	.drop_without_applying_deltas();
	assert_eq!(project(app).folder().as_str(), DOCS);
}

fn raw_input() -> RawInput {
	RawInput {
		screen_rect: Some(Rect::from_min_size(Pos2::ZERO, crate::MINIMUM_SIZE.into())),
		..Default::default()
	}
}

#[derive(Debug)]
struct TestDrop(PathBuf);

impl DroppedFile for TestDrop {
	fn path(&self) -> &Path {
		&self.0
	}

	fn bytes(&self) -> Result<Vec<u8>, String> {
		panic!("the project editor must never request dropped payload bytes")
	}
}

fn drop_paths(ctx: &Context, app: &mut App, paths: impl IntoIterator<Item = PathBuf>) {
	let dropped_files = paths
		.into_iter()
		.map(|path| -> DroppedFileHandle { Arc::new(TestDrop(path)) })
		.collect();
	ctx.run_ui(
		RawInput {
			dropped_files,
			..raw_input()
		},
		|_| {
			app.import_dropped_files(ctx);
			assert!(ctx.input(|input| input.raw.dropped_files.is_empty()));
		},
	)
	.drop_without_applying_deltas();
}
