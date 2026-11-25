use std::{
	fs,
	os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
	path::{Path, PathBuf},
	time::Duration,
};

use dmgez_config::project_edit::ProjectEdit;

use super::{super::open_outcome, *};
use crate::project::ProjectTab;

const SOURCE: &str = "# keep this comment\nschema_version = 1\n\n[image]\nname = 'Original' # label\noutput = 'dist/example.dmg'\n";
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const ORDINARY_MODE: u32 = 0o640;
const MODE_BITS: u32 = 0o777;

fn opened(path: &Path, ctx: &Context) -> App {
	fs::write(path, SOURCE).unwrap();
	let mut app = App::new(ctx, None);
	app.finish(ctx, open_outcome(path.to_owned()));
	app
}

fn project(app: &mut App) -> &mut ProjectTab {
	let Opened {
		document: Document::Project(project),
		..
	} = app.opened.as_mut().unwrap()
	else {
		panic!("expected project")
	};
	project
}

fn edit(app: &mut App, name: &str) -> String {
	let project = project(app);
	project.apply_edits(&[ProjectEdit::SetImageName(name.to_owned())]);
	project.document.source().to_owned()
}

fn receive(app: &mut App, ctx: &Context) {
	let worker = app.worker.take().expect("expected save worker");
	let result = worker.receiver.recv_timeout(WORKER_TIMEOUT).unwrap();
	app.finish(ctx, result);
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().canonicalize().unwrap().join("dmg.toml");
	(scratch, path)
}

#[test]
fn save_retains_exact_edited_text_and_rebinds_the_next_save() {
	let (_scratch, path) = fixture();
	let ctx = Context::default();
	let mut app = opened(&path, &ctx);
	fs::set_permissions(&path, fs::Permissions::from_mode(ORDINARY_MODE)).unwrap();
	let incumbent = fs::metadata(&path).unwrap().ino();

	let first = edit(&mut app, "Edited");
	assert!(first.starts_with("# keep this comment\n"));
	assert!(first.contains("# label"));
	app.save_project(&ctx);
	receive(&mut app, &ctx);
	assert_eq!(app.error, None);
	assert!(!project(&mut app).is_dirty());
	assert_eq!(fs::read_to_string(&path).unwrap(), first);
	let published = fs::metadata(&path).unwrap();
	assert_ne!(published.ino(), incumbent);
	assert_eq!(published.permissions().mode() & MODE_BITS, ORDINARY_MODE);

	let second = edit(&mut app, "Edited again");
	app.save_project(&ctx);
	receive(&mut app, &ctx);
	assert_eq!(app.error, None);
	assert_eq!(fs::read_to_string(&path).unwrap(), second);
	assert!(!project(&mut app).is_dirty());
	assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn disk_content_conflict_keeps_edits_and_save_copy_rebinds_the_identity() {
	let (_scratch, path) = fixture();
	let ctx = Context::default();
	let mut app = opened(&path, &ctx);
	let first = edit(&mut app, "Local edits");
	let external = SOURCE.replace("Original", "External");
	fs::write(&path, &external).unwrap();
	app.save_project(&ctx);
	receive(&mut app, &ctx);
	assert!(
		app.error
			.as_deref()
			.is_some_and(|error| error.contains("changed on disk"))
	);
	assert!(project(&mut app).is_dirty());
	assert_eq!(project(&mut app).document.source(), first);
	assert_eq!(fs::read_to_string(&path).unwrap(), external);

	const COPY: &str = "resolved.toml";
	app.copy_name = COPY.to_owned();
	app.write_copy(&ctx);
	receive(&mut app, &ctx);
	let copy = path.with_file_name(COPY);
	assert_eq!(app.opened.as_ref().unwrap().path, copy);
	assert_eq!(fs::read_to_string(&copy).unwrap(), first);
	let second = edit(&mut app, "Copy changes");
	app.save_project(&ctx);
	receive(&mut app, &ctx);
	assert_eq!(app.error, None);
	assert_eq!(fs::read_to_string(copy).unwrap(), second);
	assert_eq!(fs::read_to_string(path).unwrap(), external);
}

#[test]
fn save_refuses_replaced_inode_and_symlink_even_when_text_matches() {
	let (_scratch, path) = fixture();
	let ctx = Context::default();
	let mut app = opened(&path, &ctx);
	let original = path.with_file_name("renamed.toml");
	fs::rename(&path, &original).unwrap();
	fs::write(&path, SOURCE).unwrap();
	edit(&mut app, "Local edits");
	for use_symlink in [false, true] {
		if use_symlink {
			fs::remove_file(&path).unwrap();
			symlink(&original, &path).unwrap();
		}
		app.save_project(&ctx);
		receive(&mut app, &ctx);
		assert!(
			app.error
				.as_deref()
				.is_some_and(|error| error.contains("changed on disk"))
		);
		assert_eq!(fs::read_to_string(&path).unwrap(), SOURCE);
		assert!(project(&mut app).is_dirty());
	}
	assert_eq!(fs::read_to_string(&original).unwrap(), SOURCE);
}
