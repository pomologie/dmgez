use super::arrangement::{Alignment, Axis};
use super::*;

const SAMPLE: &str = "# original\nschema_version=1\n[image]\nname='Test'\noutput='test.dmg'\n[[contents]]\nsource='unopened.txt'\ndestination='/Docs/Test.txt'\nposition=[20,30]\n";

#[test]
fn navigates_implied_folder_and_undoes_a_saved_position() {
	let mut tab = ProjectTab::new(ProjectDocument::open(SAMPLE).unwrap());
	assert_eq!(
		tab.layout().unwrap().items()[0].destination.as_str(),
		"/Docs"
	);
	tab.navigate("/Docs".parse().unwrap());
	let target = tab.layout().unwrap().items()[0].destination.clone();
	tab.set_position(target, [98.0, 65.0]);
	assert!(tab.is_dirty());
	assert_eq!(tab.layout().unwrap().items()[0].center.x(), 98.0);
	assert!(tab.document.source().starts_with("# original"));
	tab.undo();
	assert!(!tab.is_dirty());
	tab.redo();
	let revision = tab.document.revision();
	tab.mark_saved(revision, tab.document.source().to_owned(), None);
	assert!(!tab.is_dirty());
	assert_eq!(tab.layout().unwrap().items()[0].center.y(), 65.0);

	let added = ProjectEdit::Add(dmgez_config::project_edit::ContentDraft::Copy {
		source: "second.txt".to_owned(),
		destination: Some("/Docs/second.txt".parse().unwrap()),
	});
	tab.apply_edits(&[added]);
	assert_eq!(tab.layout().unwrap().items().len(), 2);
	tab.undo();
	assert_eq!(tab.layout().unwrap().items().len(), 1);
	tab.redo();
	assert_eq!(tab.layout().unwrap().items().len(), 2);
}

#[test]
fn reflects_a_renamed_destination_without_changing_the_content_count() {
	let mut tab = ProjectTab::new(ProjectDocument::open(SAMPLE).unwrap());
	let target = "/Docs/Test.txt".parse().unwrap();
	let destination: VirtualPath = "/Notes/Test.txt".parse().unwrap();
	let mut changes = ContentChanges::default();
	changes.destination = Some(destination.clone());
	tab.apply_edits(&[ProjectEdit::Modify { target, changes }]);

	assert_eq!(
		tab.layout().unwrap().items()[0].destination.as_str(),
		"/Notes"
	);
	tab.navigate("/Notes".parse().unwrap());
	assert_eq!(tab.layout().unwrap().items()[0].destination, &destination);
}

const ARRANGE_PROJECT: &str = r#"# keep the manifest comment
schema_version = 1
[image]
name = 'Example'
output = 'example.dmg'
[[contents]]
type = 'directory'
destination = '/Docs/A'
position = [21, 32] # A stays editable
[[contents]]
type = 'directory'
destination = '/Docs/B'
position = [48, 47]
[[contents]]
type = 'directory'
destination = '/Docs/C'
position = [120, 91]
[[contents]]
type = 'directory'
destination = '/Docs/Hidden'
position = [0, 0]
hidden = true
[[contents]]
type = 'directory'
destination = '/Docs/Locked'
[[contents]]
type = 'directory'
destination = '/Other/C'
position = [90, 90]
[[finder.folders]]
path = '/Docs'
[finder.folders.icon_view.positions]
Locked = [84, 33]
"#;

fn select(tab: &mut ProjectTab, path: &str, toggle: bool) {
	tab.handle_action(Action::Select {
		path: path.parse().unwrap(),
		toggle,
	});
}

fn position(tab: &ProjectTab, path: &str) -> Option<[f64; 2]> {
	tab.document
		.project()
		.contents
		.iter()
		.find(|entry| entry.destination.as_str() == path)?
		.position
		.map(|point| point.map(dmgez_config::Coordinate::get))
}

#[test]
fn arranges_selected_folder_icons_in_single_undo_groups() {
	let mut tab = ProjectTab::new(ProjectDocument::open(ARRANGE_PROJECT).unwrap());
	tab.navigate("/Docs".parse().unwrap());
	select(&mut tab, "/Docs/A", false);
	select(&mut tab, "/Docs/B", true);
	select(&mut tab, "/Docs/C", true);

	tab.arrange(Arrangement::Align(Axis::Horizontal, Alignment::Center));
	assert_eq!(position(&tab, "/Docs/A"), Some([71.0, 32.0]));
	assert_eq!(position(&tab, "/Docs/B"), Some([71.0, 47.0]));
	assert_eq!(position(&tab, "/Docs/C"), Some([71.0, 91.0]));
	assert_eq!(position(&tab, "/Other/C"), Some([90.0, 90.0]));
	assert!(tab.document.source().contains("# A stays editable"));
	tab.undo();
	assert_eq!(tab.document.source(), ARRANGE_PROJECT);

	tab.arrange(Arrangement::Distribute(Axis::Vertical));
	assert_eq!(position(&tab, "/Docs/A"), Some([21.0, 32.0]));
	assert_eq!(position(&tab, "/Docs/B"), Some([48.0, 62.0]));
	assert_eq!(position(&tab, "/Docs/C"), Some([120.0, 91.0]));
	let distributed = tab.document.source().to_owned();
	let revision = tab.document.revision();
	tab.arrange(Arrangement::Distribute(Axis::Vertical));
	assert_eq!(tab.document.revision(), revision);

	tab.arrange(Arrangement::Nudge([1.0, -10.0]));
	assert_eq!(position(&tab, "/Docs/A"), Some([22.0, 22.0]));
	assert_eq!(position(&tab, "/Docs/B"), Some([49.0, 52.0]));
	assert_eq!(position(&tab, "/Docs/C"), Some([121.0, 81.0]));
	tab.undo();
	assert_eq!(tab.document.source(), distributed);
	tab.redo();
	assert_eq!(position(&tab, "/Docs/B"), Some([49.0, 52.0]));
	tab.navigate("/Other".parse().unwrap());
	assert!(tab.selected.is_empty());
}

#[test]
fn refuses_partial_hidden_locked_and_out_of_range_arrangements() {
	let mut tab = ProjectTab::new(ProjectDocument::open(ARRANGE_PROJECT).unwrap());
	tab.navigate("/Docs".parse().unwrap());
	select(&mut tab, "/Docs/A", false);
	select(&mut tab, "/Docs/Locked", true);
	tab.arrange(Arrangement::Nudge([1.0, 0.0]));
	assert_eq!(tab.document.source(), ARRANGE_PROJECT);

	select(&mut tab, "/Docs/Locked", true);
	select(&mut tab, "/Docs/Hidden", true);
	tab.arrange(Arrangement::Nudge([1.0, 0.0]));
	assert_eq!(tab.document.source(), ARRANGE_PROJECT);

	tab.show_hidden = true;
	tab.arrange(Arrangement::Nudge([-1.0, 0.0]));
	assert_eq!(tab.document.source(), ARRANGE_PROJECT);
	assert!(tab.error.as_deref().unwrap().contains("/Docs/Hidden"));

	tab.arrange(Arrangement::Nudge([1.0, 0.0]));
	assert_eq!(position(&tab, "/Docs/A"), Some([22.0, 32.0]));
	assert_eq!(position(&tab, "/Docs/Hidden"), Some([1.0, 0.0]));
	assert!(tab.error.is_none());
}

#[test]
fn alignment_pins_unchanged_grid_anchors_with_other_selected_icons() {
	const GRID: &str = r#"schema_version = 1
[image]
name = 'Example'
output = 'example.dmg'
[[contents]]
type = 'directory'
destination = '/A'
[[contents]]
type = 'directory'
destination = '/B'
[[contents]]
type = 'directory'
destination = '/C'
"#;

	let mut tab = ProjectTab::new(ProjectDocument::open(GRID).unwrap());
	let layout = tab.layout().unwrap();
	let expected = [layout.items()[2].center.x(), layout.items()[2].center.y()];
	select(&mut tab, "/B", false);
	select(&mut tab, "/C", true);
	tab.arrange(Arrangement::Align(Axis::Horizontal, Alignment::End));

	assert_eq!(position(&tab, "/A"), None);
	assert_eq!(position(&tab, "/B").unwrap()[0], expected[0]);
	assert_eq!(position(&tab, "/C"), Some(expected));
	tab.undo();
	assert_eq!(tab.document.source(), GRID);
}
