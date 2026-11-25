use std::{fs, time::Duration};

use dmgez_config::project_edit::ProjectEdit;
use eframe::egui::{Event, Pos2, RawInput, Rect, accesskit};

use crate::document::Document;

use super::*;

const PROJECT: &str = "schema_version=1\n[image]\nname='Recent'\noutput='build.dmg'\n";
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

fn entry(path: &Path) -> Entry {
	Entry {
		path: path.as_os_str().to_owned(),
		kind: Kind::Project,
	}
}

#[test]
fn reopens_a_bounded_ordered_history_and_persists_opt_out() {
	let directory = tempfile::tempdir().unwrap();
	let store = Store::at(directory.path().join("prefs"));
	let paths: Vec<_> = (0..MAX_ENTRIES + 2)
		.map(|index| entry(&directory.path().join(format!("{index}.toml"))))
		.collect();
	for path in &paths {
		store.update(Change::Opened(path.clone())).unwrap();
	}
	let preferences = store.load().unwrap();
	assert_eq!(
		preferences.entries,
		paths
			.iter()
			.rev()
			.take(MAX_ENTRIES)
			.cloned()
			.collect::<Vec<_>>(),
	);
	let reopened = paths[3].clone();
	let preferences = store.update(Change::Opened(reopened.clone())).unwrap();
	assert_eq!(preferences.entries.first(), Some(&reopened));
	assert_eq!(preferences.entries.len(), MAX_ENTRIES);

	let second_instance = store.clone();
	let disabled = store.update(Change::Remember(false)).unwrap();
	assert!(!disabled.remember);
	assert!(disabled.entries.is_empty());
	assert_eq!(
		second_instance
			.update(Change::Opened(reopened.clone()))
			.unwrap(),
		disabled
	);
	assert_eq!(second_instance.load().unwrap(), disabled);

	let enabled = second_instance.update(Change::Remember(true)).unwrap();
	assert!(enabled.remember);
	assert!(enabled.entries.is_empty());
	let preferences = store.update(Change::Opened(reopened)).unwrap();
	assert_eq!(preferences.entries.len(), 1);
	let cleared = store.update(Change::Clear).unwrap();
	assert!(cleared.remember);
	assert!(cleared.entries.is_empty());
	assert_eq!(store.load().unwrap(), cleared);
}

#[cfg(unix)]
#[test]
fn persists_exact_native_paths() {
	use std::os::unix::ffi::OsStringExt as _;

	let directory = tempfile::tempdir().unwrap();
	let store = Store::at(directory.path().join("prefs"));
	let path = directory
		.path()
		.join(OsString::from_vec(b"nonutf8\xff.toml".to_vec()));
	let original = entry(&path);
	store.update(Change::Opened(original.clone())).unwrap();
	let preferences = store.load().unwrap();
	assert_eq!(preferences.entries, [original]);
}

#[test]
fn accessible_recent_menu_opens_known_files_and_protects_unsaved_changes() {
	let directory = tempfile::tempdir().unwrap();
	let ctx = Context::default();
	ctx.enable_accesskit();
	let first = directory.path().join("first.toml");
	let second = directory.path().join("second.toml");
	fs::write(&first, PROJECT).unwrap();
	fs::write(&second, PROJECT).unwrap();
	let store = Store::at(directory.path().join("prefs"));
	let first_entry = entry(&first.canonicalize().unwrap());
	store.update(Change::Opened(first_entry.clone())).unwrap();
	let mut app = App::new(&ctx, None);
	app.finish_recent_load(&ctx, Loaded::open(store.clone()), Some(second.clone()));
	receive(&mut app, &ctx);
	receive(&mut app, &ctx);
	let Document::Project(project) = &mut app.opened.as_mut().unwrap().document else {
		panic!("the startup worker should open the second project");
	};
	project.apply_edits(&[ProjectEdit::SetImageName("Unsaved".into())]);
	assert!(app.is_dirty());
	app.finish(&ctx, open_outcome(second.with_extension("bad")));
	assert_eq!(store.load().unwrap().entries.len(), 2);

	let nodes = frame(&ctx, &mut app, vec![]);
	frame(&ctx, &mut app, vec![click(&nodes, MENU_LABEL)]);
	// egui sizes the popup while it is invisible on its first frame.
	let nodes = frame(&ctx, &mut app, vec![]);
	assert!(
		nodes
			.iter()
			.any(|(_, node)| node.label() == Some(REMEMBER_LABEL))
	);
	frame(
		&ctx,
		&mut app,
		vec![click(&nodes, &first_entry.path().display().to_string())],
	);
	assert!(
		matches!(&app.confirm, Some(Pending::OpenRecent(selected)) if selected == &first_entry)
	);
	assert!(app.worker.is_none());
	assert!(app.is_dirty());

	let Some(Pending::OpenRecent(selected)) = app.confirm.take() else {
		panic!("recent-file selection should await discard confirmation");
	};
	app.open_recent(&ctx, selected);
	receive(&mut app, &ctx);
	receive(&mut app, &ctx);
	assert_eq!(app.opened.as_ref().unwrap().path, first_entry.path());
	assert!(!app.is_dirty());
	assert_eq!(store.load().unwrap().entries.first(), Some(&first_entry));

	let nodes = frame(&ctx, &mut app, vec![]);
	frame(&ctx, &mut app, vec![click(&nodes, MENU_LABEL)]);
	let nodes = frame(&ctx, &mut app, vec![]);
	frame(&ctx, &mut app, vec![click(&nodes, REMEMBER_LABEL)]);
	receive(&mut app, &ctx);
	let disabled = store.load().unwrap();
	assert!(!disabled.remember);
	assert!(disabled.entries.is_empty());
}

fn receive(app: &mut App, ctx: &Context) {
	let worker = app.worker.take().expect("expected document worker");
	let completion = worker.receiver.recv_timeout(WORKER_TIMEOUT).unwrap();
	app.finish(ctx, completion);
}

fn frame(
	ctx: &Context,
	app: &mut App,
	events: Vec<Event>,
) -> Vec<(accesskit::NodeId, accesskit::Node)> {
	let mut output = ctx.run_ui(
		RawInput {
			screen_rect: Some(Rect::from_min_size(Pos2::ZERO, crate::MINIMUM_SIZE.into())),
			events,
			..Default::default()
		},
		|ui| app.toolbar(ui),
	);
	let nodes = output
		.platform_output
		.accesskit_update
		.take()
		.unwrap()
		.nodes;
	output.drop_without_applying_deltas();
	nodes
}

fn click(nodes: &[(accesskit::NodeId, accesskit::Node)], label: &str) -> Event {
	let (id, node) = nodes
		.iter()
		.find(|(_, node)| node.label() == Some(label))
		.unwrap_or_else(|| panic!("missing accessible control {label:?}"));
	assert!(node.supports_action(accesskit::Action::Click), "{label}");
	Event::AccessKitActionRequest(accesskit::ActionRequest {
		action: accesskit::Action::Click,
		target_tree: accesskit::TreeId::ROOT,
		target_node: *id,
		data: None,
	})
}
