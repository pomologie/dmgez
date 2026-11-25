use dmgez_config::{Payload, project_edit::ProjectDocument};
use eframe::egui::{Context, Event, Pos2, RawInput, Rect, accesskit};

use super::*;

const SOURCE: &str = r#"# keep this project comment
schema_version = 1
[image]
name = 'Example'
output = 'example.dmg'
[[contents]]
source = 'unopened.txt'
destination = '/Docs/Test.txt' # keep this content comment
"#;

const DOCS: &str = "/Docs";
const NEW_FOLDER: &str = "Release Assets";
const NEW_FOLDER_PATH: &str = "/Docs/Release Assets";
const RELATIVE_TARGET: &str = "../Test.txt";

fn payload<'a>(tab: &'a ProjectTab, path: &str) -> &'a Payload {
	&tab.document
		.project()
		.contents
		.iter()
		.find(|content| content.destination.as_str() == path)
		.unwrap()
		.payload
}

#[test]
fn creates_folder_local_entries_and_undo_returns_to_a_surviving_folder() {
	let mut tab = ProjectTab::new(ProjectDocument::open(SOURCE).unwrap());
	tab.navigate(DOCS.parse().unwrap());
	tab.create_entry(NEW_FOLDER, None).unwrap();
	assert!(matches!(payload(&tab, NEW_FOLDER_PATH), Payload::Directory));
	assert_eq!(tab.selected.first().unwrap().as_str(), NEW_FOLDER_PATH);
	let with_directory = tab.document.source().to_owned();

	tab.navigate(NEW_FOLDER_PATH.parse().unwrap());
	tab.create_entry("Release notes", Some(RELATIVE_TARGET))
		.unwrap();
	assert!(matches!(
		payload(&tab, "/Docs/Release Assets/Release notes"),
		Payload::Symlink { target } if target.as_str() == RELATIVE_TARGET
	));
	assert!(
		tab.document
			.source()
			.contains("# keep this project comment")
	);
	assert!(
		tab.document
			.source()
			.contains("# keep this content comment")
	);

	tab.undo();
	assert_eq!(tab.document.source(), with_directory);
	assert_eq!(tab.folder().as_str(), NEW_FOLDER_PATH);
	assert!(tab.selected.is_empty());
	tab.undo();
	assert_eq!(tab.document.source(), SOURCE);
	assert_eq!(tab.folder().as_str(), DOCS);
	tab.redo();
	assert!(matches!(payload(&tab, NEW_FOLDER_PATH), Payload::Directory));
}

#[test]
fn rejects_invalid_names_targets_and_conflicts_without_editing() {
	let mut tab = ProjectTab::new(ProjectDocument::open(SOURCE).unwrap());
	let revision = tab.document.revision();
	for name in [
		"",
		".",
		"..",
		"/outside",
		"sub/item",
		"sub\\item",
		"x:meta",
		"a\tb",
	] {
		assert!(matches!(
			tab.create_entry(name, None),
			Err(CreateError::Name)
		));
	}
	assert!(matches!(
		tab.create_entry("Docs", None),
		Err(CreateError::Conflict(name)) if name == "Docs"
	));
	assert!(matches!(
		tab.create_entry(".DS_Store", None),
		Err(CreateError::Edit(_))
	));
	tab.navigate(DOCS.parse().unwrap());
	assert!(matches!(
		tab.create_entry("Test.txt", Some(RELATIVE_TARGET)),
		Err(CreateError::Conflict(name)) if name == "Test.txt"
	));
	assert!(matches!(
		tab.create_entry("new link", Some("")),
		Err(CreateError::Edit(_))
	));
	assert_eq!(tab.document.revision(), revision);
	assert_eq!(tab.document.source(), SOURCE);
}

#[test]
fn refuses_interpolation_that_could_change_the_folder_or_link_target() {
	const WITH_DEFINES: &str = r#"
schema_version = 1
[defines]
RAW_FOLDER = "${OTHER}"
OTHER = "different folder"
[image]
name = "Example"
output = "example.dmg"
[[contents]]
type = "directory"
destination = "/${RAW_FOLDER}"
"#;
	let mut tab = ProjectTab::new(ProjectDocument::open(WITH_DEFINES).unwrap());
	assert!(matches!(
		tab.create_entry("${OTHER}", None),
		Err(CreateError::VariableSyntax)
	));
	assert!(matches!(
		tab.create_entry("link", Some("${OTHER}")),
		Err(CreateError::VariableSyntax)
	));
	tab.navigate("/${OTHER}".parse().unwrap());
	assert!(matches!(
		tab.create_entry("child", None),
		Err(CreateError::VariableSyntax)
	));
	assert_eq!(tab.document.source(), WITH_DEFINES);
}

#[test]
fn accessible_add_menu_creates_and_cancels_through_the_contents_ui() {
	let ctx = Context::default();
	ctx.enable_accesskit();
	let mut tab = ProjectTab::new(ProjectDocument::open(SOURCE).unwrap());
	tab.navigate(DOCS.parse().unwrap());

	choose(&ctx, &mut tab, MENU[0].0);
	frame(&ctx, &mut tab, vec![]);
	frame(&ctx, &mut tab, vec![Event::Text(NEW_FOLDER.to_owned())]);
	activate(&ctx, &mut tab, "Create");
	assert!(matches!(payload(&tab, NEW_FOLDER_PATH), Payload::Directory));
	let revision = tab.document.revision();

	choose(&ctx, &mut tab, MENU[1].0);
	frame(&ctx, &mut tab, vec![]);
	frame(&ctx, &mut tab, vec![Event::Text("Unused".to_owned())]);
	activate(&ctx, &mut tab, "Create");
	assert!(tab.entry_draft.as_ref().unwrap().error.is_some());
	activate(&ctx, &mut tab, "Cancel");
	assert!(tab.entry_draft.is_none());
	assert_eq!(tab.document.revision(), revision);

	choose(&ctx, &mut tab, MENU[1].0);
	frame(&ctx, &mut tab, vec![]);
	let nodes = frame(&ctx, &mut tab, vec![Event::Text("Test link".to_owned())]);
	let (label, _) = nodes
		.iter()
		.find(|(_, node)| node.value() == Some("Link target"))
		.unwrap();
	let (target, node) = nodes
		.iter()
		.find(|(_, node)| node.labelled_by().contains(label))
		.unwrap();
	assert!(node.supports_action(accesskit::Action::Focus));
	frame(
		&ctx,
		&mut tab,
		vec![request(accesskit::Action::Focus, *target)],
	);
	frame(
		&ctx,
		&mut tab,
		vec![Event::Text(RELATIVE_TARGET.to_owned())],
	);
	activate(&ctx, &mut tab, "Create");
	assert!(matches!(
		payload(&tab, "/Docs/Test link"),
		Payload::Symlink { target } if target.as_str() == RELATIVE_TARGET
	));
	let revision = tab.document.revision();

	choose(&ctx, &mut tab, MENU[2].0);
	assert!(matches!(
		payload(&tab, "/Docs/Applications"),
		Payload::Symlink { target } if target.as_str() == APPLICATIONS_TARGET
	));
	assert_eq!(tab.selected.first().unwrap().as_str(), "/Docs/Applications");
	tab.undo();
	assert_eq!(tab.document.revision(), revision);
	assert!(tab.selected.is_empty());
}

fn choose(ctx: &Context, tab: &mut ProjectTab, label: &str) {
	activate(ctx, tab, "Add entry");
	activate(ctx, tab, label);
}

fn activate(ctx: &Context, tab: &mut ProjectTab, label: &str) {
	let nodes = frame(ctx, tab, vec![]);
	let (id, node) = nodes
		.iter()
		.find(|(_, node)| node.label() == Some(label))
		.unwrap_or_else(|| panic!("missing accessible control {label:?}"));
	assert!(node.supports_action(accesskit::Action::Click), "{label}");
	frame(ctx, tab, vec![request(accesskit::Action::Click, *id)]);
}

fn request(action: accesskit::Action, target_node: accesskit::NodeId) -> Event {
	Event::AccessKitActionRequest(accesskit::ActionRequest {
		action,
		target_tree: accesskit::TreeId::ROOT,
		target_node,
		data: None,
	})
}

fn frame(
	ctx: &Context,
	tab: &mut ProjectTab,
	events: Vec<Event>,
) -> Vec<(accesskit::NodeId, accesskit::Node)> {
	let mut output = ctx.run_ui(
		RawInput {
			screen_rect: Some(Rect::from_min_size(Pos2::ZERO, crate::MINIMUM_SIZE.into())),
			events,
			..Default::default()
		},
		|ui| tab.ui(ui),
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
