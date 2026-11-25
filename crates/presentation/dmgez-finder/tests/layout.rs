//! Finder record checks for the basic layout portion of D-FINDER-01.

use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmgez_config::{FinderOptions, Project};
use dmgez_finder::{Error, LayoutItem, validate_options, write_store};
use ds_parser::{DSStore, FieldKey, FinderViewStyle, IconLocation, Value, WindowBounds};

const PROJECT: &str = r##"
schema_version = 1
[image]
name = "Finder Oracle"
output = "Finder Oracle.dmg"
[[contents]]
type = "directory"
destination = "/Applications"
position = [140, 120]
[[contents]]
type = "directory"
destination = "/Release Notes"
[finder]
window_position = [92, -140]
window_size = [600, 260]
sidebar_width = 125
show_status_bar = true
show_icon_preview = true
show_item_info = false
background = { kind = "color", value = "#246" }
[finder.icon_view]
grid_offset = [5, -3]
grid_spacing = 84
scroll_position = [2, 3]
icon_size = 96
text_size = 15
label_position = "right"
[finder.icon_view.positions]
"Release Notes" = [450, 125]
"##;
const APPLICATIONS: &str = "Applications";
const RELEASE_NOTES: &str = "Release Notes";
const CURRENT_FOLDER: &str = ".";
const ROOT_OPTIONS_PROJECT: &str = r#"
schema_version = 1
[image]
name = "Finder Options"
output = "Finder Options.dmg"
[finder]
"#;
const ORACLE: &str = include_str!("oracle.py");

fn fixture_store(reverse: bool) -> Vec<u8> {
	let project = Project::parse(PROJECT).unwrap();
	let finder = project.finder.as_ref().unwrap();
	let mut items = [
		LayoutItem {
			name: APPLICATIONS,
			position: project.contents[0].position,
		},
		LayoutItem {
			name: RELEASE_NOTES,
			position: None,
		},
	];
	if reverse {
		items.reverse();
	}
	write_store(Some(&finder.options), &items).unwrap()
}

fn record<'a>(store: &'a DSStore, name: &str, field: FieldKey) -> &'a Value {
	&store
		.records
		.iter()
		.find(|record| record.name == name && record.field == field)
		.unwrap()
		.value
}

fn options(settings: &str) -> FinderOptions {
	Project::parse(&format!("{ROOT_OPTIONS_PROJECT}{settings}"))
		.unwrap()
		.finder
		.unwrap()
		.options
}

#[test]
fn writes_deterministic_folder_and_icon_records() {
	let bytes = fixture_store(false);
	assert_eq!(bytes, fixture_store(true));
	let store = ds_parser::parse(&bytes).unwrap();
	assert!(store.warnings.is_empty(), "{:?}", store.warnings);
	assert_eq!(store.records.len(), 6);
	assert_eq!(
		record(&store, CURRENT_FOLDER, FieldKey::icvl),
		&Value::Type(FinderViewStyle::Icon.fourcc())
	);
	assert_eq!(
		record(&store, CURRENT_FOLDER, FieldKey::vSrn),
		&Value::Long(1)
	);
	let Value::BrowserWindowSettings(window) = record(&store, CURRENT_FOLDER, FieldKey::bwsp)
	else {
		panic!("expected Finder browser window property list");
	};
	assert_eq!(
		window.window_bounds,
		Some(WindowBounds {
			x: 92.0,
			y: -140.0,
			width: 600.0,
			height: 260.0
		})
	);
	assert_eq!(window.show_status_bar, Some(true));
	assert_eq!(window.show_toolbar, Some(false));
	assert_eq!(window.sidebar_width, Some(125.0));
	let Value::IconViewProperties(icons) = record(&store, CURRENT_FOLDER, FieldKey::icvp) else {
		panic!("expected Finder icon-view property list");
	};
	assert_eq!(icons.background_type, Some(1));
	assert_eq!(
		icons.background_color,
		Some((34.0 / 255.0, 68.0 / 255.0, 102.0 / 255.0))
	);
	assert_eq!(icons.grid_offset, Some((5.0, -3.0)));
	assert_eq!(icons.label_on_bottom, Some(false));
	assert_eq!(icons.show_icon_preview, Some(true));
	assert_eq!(icons.show_item_info, Some(false));
	assert_eq!(
		record(&store, APPLICATIONS, FieldKey::Iloc),
		&Value::IconLocation(IconLocation::new(140, 120))
	);
	assert_eq!(
		record(&store, RELEASE_NOTES, FieldKey::Iloc),
		&Value::IconLocation(IconLocation::new(450, 125))
	);
}

#[test]
fn defaults_are_present_without_user_finder_settings() {
	let bytes = write_store(None, &[]).unwrap();
	let store = ds_parser::parse(&bytes).unwrap();
	let Value::BrowserWindowSettings(window) = record(&store, CURRENT_FOLDER, FieldKey::bwsp)
	else {
		panic!("expected Finder browser window property list");
	};
	assert_eq!(
		window.window_bounds,
		Some(WindowBounds {
			x: 100.0,
			y: 100.0,
			width: 640.0,
			height: 280.0
		})
	);
	let Value::IconViewProperties(icons) = record(&store, CURRENT_FOLDER, FieldKey::icvp) else {
		panic!("expected Finder icon-view property list");
	};
	assert_eq!(icons.background_type, Some(0));
	assert_eq!(icons.grid_spacing, Some(80.0));
	assert_eq!(icons.icon_size, Some(128.0));
}

#[test]
fn refuses_settings_that_cannot_be_preserved() {
	assert!(matches!(
		validate_options(Some(&options("default_view = 'column-view'"))),
		Err(Error::View(_))
	));
	assert!(matches!(
		validate_options(Some(&options("include_icon_view_settings = false"))),
		Err(Error::IconViewExcluded)
	));
	assert!(matches!(
		validate_options(Some(&options(
			"default_view = 'list-view'\ninclude_list_view_settings = false"
		))),
		Err(Error::ListViewExcluded)
	));
	assert!(matches!(
		validate_options(Some(&options(
			"background = { kind = 'image', source = 'background.png' }"
		))),
		Err(Error::ImageBackground)
	));
	assert!(matches!(
		validate_options(Some(&options(
			"background = { kind = 'color', value = '#abcd' }"
		))),
		Err(Error::BackgroundColor)
	));
	assert!(matches!(
		validate_options(Some(&options(
			"[finder.icon_view]\narrange_by = 'comments'"
		))),
		Err(Error::Arrangement(_))
	));
}

#[test]
fn refuses_missing_conflicting_or_unrepresentable_item_positions() {
	let project = Project::parse(PROJECT).unwrap();
	let finder_options = &project.finder.unwrap().options;
	assert!(
		matches!(write_store(Some(finder_options), &[]), Err(Error::MissingItem(name)) if name == RELEASE_NOTES)
	);

	let position = project.contents[0].position;
	for (name, expect) in [(RELEASE_NOTES, true), (APPLICATIONS, false)] {
		let items = [
			LayoutItem { name, position },
			LayoutItem {
				name: APPLICATIONS,
				position,
			},
		];
		let result = write_store(Some(finder_options), &items);
		if expect {
			assert!(matches!(result, Err(Error::DuplicatePosition(name)) if name == RELEASE_NOTES));
		} else {
			assert!(matches!(result, Err(Error::DuplicateName(name)) if name == APPLICATIONS));
		}
	}

	let options = options("[finder.icon_view.positions]\nApplications = [5.5, 10]");
	let items = [LayoutItem {
		name: APPLICATIONS,
		position: None,
	}];
	assert!(
		matches!(write_store(Some(&options), &items), Err(Error::Position(name)) if name == APPLICATIONS)
	);
}

#[test]
fn rejects_position_records_before_crossing_the_single_leaf_budget() {
	let project = Project::parse(PROJECT).unwrap();
	let position = project.contents[0].position;
	let names: Vec<_> = (0..256)
		.map(|index| format!("Positioned Example File {index:03}"))
		.collect();
	let items: Vec<_> = names
		.iter()
		.map(|name| LayoutItem { name, position })
		.collect();
	assert!(matches!(write_store(None, &items), Err(Error::StoreBudget)));

	// The last accepted count still yields the expected untruncated record set.
	let maximum = (0..items.len())
		.rev()
		.find(|&count| write_store(None, &items[..count]).is_ok())
		.unwrap();
	let accepted = write_store(None, &items[..maximum]).unwrap();
	let decoded = ds_parser::parse(&accepted).unwrap();
	assert!(decoded.warnings.is_empty(), "{:?}", decoded.warnings);
	assert_eq!(decoded.records.len(), maximum + 4);
	assert!(matches!(
		write_store(None, &items[..=maximum]),
		Err(Error::StoreBudget)
	));
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_python_decoder_accepts_the_generated_store() {
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle
		.stdin
		.take()
		.unwrap()
		.write_all(&fixture_store(false))
		.unwrap();
	assert!(oracle.wait().unwrap().success());
}
