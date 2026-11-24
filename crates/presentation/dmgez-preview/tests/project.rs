use dmgez_config::{LabelPosition, Project, View, VirtualPath};
use dmgez_preview::{
	CanvasPoint, FolderPoint, ItemInput, ItemVisibility, LayoutError, PositionSource,
	ProjectFolder, Size,
};

const ROOT: &str = "/";
const DOCS: &str = "/Docs";
const IMAGE: &str = "schema_version=1\n[image]\nname='Geometry'\noutput='preview.dmg'\n";
const FOLDERS: &str = r##"
[[contents]]
type = "directory"
destination = "/Docs"
position = [34.25, 44.5]

[[contents]]
source = "missing-on-purpose/Notes.txt"
destination = "/Docs/Notes.txt"
hide_extension = true

[[contents]]
type = "symlink"
destination = "/Docs/Shortcut"
target = "/Applications"
position = [36.5, 47.75]
hidden = true

[finder]
window_size = [300, 200]
background = { kind = "color", value = "#303040" }

[finder.icon_view]
icon_size = 32
grid_spacing = 24
scroll_position = [5, 10]
label_position = "right"

[[finder.folders]]
path = "/docs"
window_size = [180, 128]

[finder.folders.icon_view]
text_size = 12
grid_offset = [4, 8]
positions = { "Notes.txt" = [36.5, 47.75] }
"##;

fn make_project(settings: &str) -> Project {
	Project::parse(&format!("{IMAGE}{settings}")).unwrap()
}

fn path(text: &str) -> VirtualPath {
	text.parse().unwrap()
}

#[test]
fn reads_root_and_nested_style_with_local_positions() {
	let project = make_project(FOLDERS);
	let root_item = ItemInput::from(&project.contents[0]);
	let root = ProjectFolder::from_project(&project, &path(ROOT), &[root_item]).unwrap();
	assert_eq!(root.style().window_size, Size::new(300.0, 200.0).unwrap());
	assert_eq!(
		root.items()[0].center,
		FolderPoint::new(34.25, 44.5).unwrap()
	);
	assert_eq!(root.items()[0].position_source, PositionSource::Content);

	let folder_inputs: Vec<_> = project.contents[1..].iter().map(ItemInput::from).collect();
	let folder = ProjectFolder::from_project(&project, &path(DOCS), &folder_inputs).unwrap();
	let style = folder.style();
	assert_eq!(style.window_size, Size::new(180.0, 128.0).unwrap());
	assert_eq!(style.icon_size, Size::new(32.0, 32.0).unwrap());
	assert_eq!(style.scroll, FolderPoint::new(5.0, 10.0).unwrap());
	assert_eq!(style.grid_offset, FolderPoint::new(4.0, 8.0).unwrap());
	assert_eq!(style.label_position, LabelPosition::Right);
	assert_eq!(style.grid_spacing, 24.0);
	assert_eq!(style.text_size, 12.0);
	assert!(style.background.is_some());
	let [notes, shortcut] = folder.items() else {
		panic!("expected two positioned entries")
	};
	assert_eq!(notes.name, "Notes.txt");
	assert_eq!(notes.position_source, PositionSource::FinderFolder);
	assert!(notes.hide_extension);
	assert_eq!(shortcut.position_source, PositionSource::Content);
	assert!(shortcut.hidden);
	assert!(notes.icon_bounds.intersects(shortcut.icon_bounds));
}

#[test]
fn hit_testing_uses_scroll_clipping_and_hidden_policy() {
	let project = make_project(FOLDERS);
	let children: Vec<_> = project.contents[1..].iter().map(ItemInput::from).collect();
	let folder = ProjectFolder::from_project(&project, &path(DOCS), &children).unwrap();
	let viewport = folder
		.style()
		.viewport(CanvasPoint::new(100.0, 50.0).unwrap(), 2.0)
		.unwrap();
	let pointer = viewport.to_canvas(folder.items()[0].center).unwrap();
	let item = folder
		.hit_test(pointer, viewport, ItemVisibility::Visible)
		.unwrap()
		.unwrap();
	assert_eq!(item.name, "Notes.txt");
	let item = folder
		.hit_test(pointer, viewport, ItemVisibility::IncludeHidden)
		.unwrap()
		.unwrap();
	assert_eq!(item.name, "Shortcut");

	let clipped = dmgez_preview::Viewport::new(
		CanvasPoint::new(30.0, 50.0).unwrap(),
		FolderPoint::new(40.0, 50.0).unwrap(),
		2.0,
	)
	.unwrap();
	let off_canvas = clipped.to_canvas(folder.items()[0].center).unwrap();
	assert!(
		folder
			.hit_test(off_canvas, clipped, ItemVisibility::IncludeHidden)
			.unwrap()
			.is_none()
	);
}

#[test]
fn deterministic_fallback_is_separate_from_configured_positions() {
	let project = make_project(
		"[finder]\nwindow_size=[160,100]\n[finder.icon_view]\nicon_size=32\ngrid_spacing=40\n",
	);
	let root = path(ROOT);
	let a = path("/A");
	let b = path("/B");
	let input = |destination| ItemInput {
		destination,
		position: None,
		hidden: false,
		hide_extension: false,
	};
	let layout = ProjectFolder::from_project(&project, &root, &[input(&b), input(&a)]).unwrap();
	let [a, b] = layout.items() else {
		panic!("expected fallback items")
	};
	assert_eq!((a.name, b.name), ("A", "B"));
	assert_eq!(a.center, FolderPoint::new(16.0, 16.0).unwrap());
	assert_eq!(b.center, FolderPoint::new(56.0, 16.0).unwrap());
	assert_eq!(a.position_source, PositionSource::PreviewGrid);
	assert_eq!(b.position_source, PositionSource::PreviewGrid);
}

#[test]
fn rejects_invalid_scope_and_conflicting_positions() {
	let project = make_project(FOLDERS);
	let root = path(ROOT);
	let docs = path(DOCS);
	let input = ItemInput::from(&project.contents[1]);
	assert!(matches!(
		ProjectFolder::from_project(&project, &root, &[input]),
		Err(LayoutError::NotChild(_))
	));
	assert!(matches!(
		ProjectFolder::from_project(&project, &docs, &[input, input]),
		Err(LayoutError::DuplicateItem(_))
	));
	assert!(matches!(
		ProjectFolder::from_project(&project, &docs, &[]),
		Err(LayoutError::MissingItem(_))
	));
	let conflict = ItemInput {
		position: project.contents[2].position,
		..input
	};
	assert!(matches!(
		ProjectFolder::from_project(&project, &docs, &[conflict]),
		Err(LayoutError::DuplicatePosition(_))
	));

	let project = make_project("[finder]\ndefault_view='list-view'\n");
	assert!(matches!(
		ProjectFolder::from_project(&project, &root, &[]),
		Err(LayoutError::UnsupportedView(View::ListView))
	));
}
