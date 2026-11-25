#![cfg(feature = "volume-icon")]

use std::{fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	manifest,
	planning::{self, PreparedBuild},
	preflight::{Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent},
	presentation::{Error, item_icon::ICON_FILE_NAME},
};
use dmgez_config::Project;
use dmgez_udif::writer::Cancellation;

const ICON: &[u8] =
	include_bytes!("../../../presentation/dmgez-finder/tests/fixtures/apple-iconutil.icns");
const PROJECT: &str =
	"schema_version = 1\n[image]\nname = 'Example'\noutput = 'example.dmg'\nformat = 'udro'\n";

fn prepare(document: &str, root: &Path, unicode: bool) -> Result<PreparedBuild, planning::Error> {
	let project = Project::parse(document).unwrap();
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.hfsplus_unicode = if unicode {
		Availability::Available
	} else {
		Availability::NotCompiled
	};
	capabilities.finder_custom_icon = Availability::Available;
	capabilities.finder_layout = Availability::Available;
	planning::prepare(
		&project,
		root,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
		Cancellation::default(),
	)
}

#[test]
fn captures_bounded_forks_and_reserves_only_a_folders_icon_child() {
	let scratch = tempfile::tempdir().unwrap();
	let path = fs::canonicalize(scratch.path()).unwrap();
	fs::write(path.join("icon.icns"), ICON).unwrap();
	fs::write(path.join("body"), b"local file").unwrap();
	let selection = r#"
[[contents]]
source = "body"
destination = "/Manual"
custom_icon = "icon.icns"

[[contents]]
type = "directory"
destination = "/Examples"
custom_icon = "icon.icns"
"#;
	let prepared = prepare(&format!("{PROJECT}{selection}"), &path, true).unwrap();
	let file = "/Manual".parse().unwrap();
	let folder = "/Examples".parse().unwrap();
	let resource = prepared.item_icon(&file).unwrap();
	assert!(resource.len() > ICON.len());
	assert_eq!(prepared.item_icon(&folder), Some(resource));
	assert!(prepared.item_icon_child(&file).is_none());
	let child = prepared.item_icon_child(&folder).unwrap();
	assert_eq!(child.as_str(), format!("/Examples/{ICON_FILE_NAME}"));
	assert!(prepared.item_icon_metadata_bytes() >= resource.len() * 2);
	assert_eq!(prepared.finder_asset_count(), 3);
	assert!(matches!(
		prepare(&format!("{PROJECT}{selection}"), &path, false),
		Err(planning::Error::Presentation(Error::ItemIconTarget { path })) if path == folder
	));
}

#[test]
fn rejects_a_discovered_signature_marker_and_link_before_opening_icon_paths() {
	let scratch = tempfile::tempdir().unwrap();
	let path = fs::canonicalize(scratch.path()).unwrap();
	fs::write(path.join("seal"), b"signature marker").unwrap();
	for marker in ["Contents", "Versions/A"] {
		let children = format!(
			r#"
[[contents]]
source = "seal"
destination = "/Renamed/{marker}/_CodeSignature/CodeResources"

[[contents]]
type = "directory"
destination = "/Renamed"
custom_icon = "absent.icns"
"#
		);
		assert!(matches!(
			prepare(&format!("{PROJECT}{children}"), &path, true),
			Err(planning::Error::Presentation(Error::ItemIconSignedBundle { path }))
				if path.as_str() == "/Renamed"
		));
	}
	let symlink = r#"
[[contents]]
type = "symlink"
destination = "/Link"
target = "/Applications"
custom_icon = "absent.icns"
"#;
	assert!(matches!(
		prepare(&format!("{PROJECT}{symlink}"), &path, true),
		Err(planning::Error::Presentation(Error::ItemIconTarget { path }))
			if path.as_str() == "/Link"
	));
}

#[test]
fn rejects_folder_child_entry_budget_and_an_output_alias_of_the_icon_source() {
	let scratch = tempfile::tempdir().unwrap();
	let path = fs::canonicalize(scratch.path()).unwrap();
	fs::write(path.join("icon.icns"), ICON).unwrap();
	fs::hard_link(path.join("icon.icns"), path.join("example.dmg")).unwrap();
	let selection = r#"
[[contents]]
type = "directory"
destination = "/Example"
custom_icon = "icon.icns"
"#;
	assert!(matches!(
		prepare(&format!("{PROJECT}{selection}"), &path, true),
		Err(planning::Error::Presentation(Error::ItemIconSource(
			manifest::Error::OutputSourceAlias { .. }
		)))
	));
	let quota = "\n[performance]\nmax_entries = 2\n";
	assert!(matches!(
		prepare(&format!("{PROJECT}{selection}{quota}"), &path, true),
		Err(planning::Error::Presentation(Error::EntryBudget))
	));
}
