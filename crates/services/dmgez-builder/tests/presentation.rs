//! Build-level checks for the D-FINDER-01 generated store handoff.

#![cfg(feature = "finder-write")]

use std::{num::NonZeroUsize, path::Path};

use dmgez_builder::{
	manifest::{Limits, discover},
	preflight::{
		Availability, BuildCapabilities, ExecutionProfile, OutputIntent, PreflightOptions,
		ReleaseIntent, preflight,
	},
	presentation::{Error, generate},
};
use dmgez_config::Project;
use dmgez_finder::{LayoutItem, write_store};

const HEADER: &str = r#"
schema_version = 1
[image]
name = "Finder Build"
output = "output.dmg"
format = "udro"
"#;

const CONTENTS: &str = r#"
[[contents]]
type = "directory"
destination = "/Docs"
[[contents]]
type = "directory"
destination = "/Docs/Child"
[[contents]]
type = "directory"
destination = "/Docs/Other/Grandchild"
position = [215, 137]
[[contents]]
type = "directory"
destination = "/Help"
"#;

const ROOT_STYLE: &str = r#"
[finder]
window_size = [800, 350]
show_toolbar = true
[finder.icon_view]
grid_spacing = 92
"#;

const CONFIGURED_LAYOUTS: &str = r#"
[finder.icon_view.positions]
Help = [140, 120]
[[finder.folders]]
path = "/docs"
show_toolbar = false
[finder.folders.icon_view]
icon_size = 96
[finder.folders.icon_view.positions]
Child = [175, 125]
"#;

fn project(contents: &str) -> Project {
	Project::parse(&format!("{HEADER}{contents}")).unwrap()
}

fn configured_profile(project: &Project, finder: Availability) -> ExecutionProfile {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_ascii = Availability::Available;
	capabilities.finder_icon_view = finder;
	preflight(
		project,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
	)
	.unwrap()
}

fn fixture_base() -> tempfile::TempDir {
	tempfile::tempdir().unwrap()
}

fn build(
	project: &Project,
	base: &Path,
) -> Result<Vec<dmgez_builder::presentation::GeneratedAsset>, Error> {
	let manifest = discover(project, base, Limits::default()).unwrap();
	let profile = configured_profile(project, Availability::Available);
	generate(project, &manifest, &profile)
}

#[test]
fn builds_only_relevant_folders_with_exact_names_and_scoped_positions() {
	let base = fixture_base();
	let config = project(&format!("{CONTENTS}{ROOT_STYLE}{CONFIGURED_LAYOUTS}"));
	let first = build(&config, base.path()).unwrap();
	assert_eq!(first, build(&config, base.path()).unwrap());
	assert_eq!(
		first
			.iter()
			.map(|asset| asset.path.as_str())
			.collect::<Vec<_>>(),
		["/.DS_Store", "/Docs/.DS_Store", "/Docs/Other/.DS_Store"],
	);

	let expected_root = project(&format!(
		"{ROOT_STYLE}[finder.icon_view.positions]\nHelp = [140, 120]\n"
	));
	let expected_docs = project(
		r#"
[finder]
window_size = [800, 350]
show_toolbar = false
[finder.icon_view]
grid_spacing = 92
icon_size = 96
[finder.icon_view.positions]
Child = [175, 125]
"#,
	);
	let expected_implicit = project(ROOT_STYLE);
	let position = config.contents[2].position;
	let scenarios = [
		(
			expected_root,
			LayoutItem {
				name: "Help",
				position: None,
			},
		),
		(
			expected_docs,
			LayoutItem {
				name: "Child",
				position: None,
			},
		),
		(
			expected_implicit,
			LayoutItem {
				name: "Grandchild",
				position,
			},
		),
	];
	for (asset, (expected, item)) in first.iter().zip(scenarios) {
		let finder = expected.finder.as_ref().unwrap();
		assert_eq!(
			asset.bytes,
			write_store(Some(&finder.options), &[item]).unwrap()
		);
	}
}

#[test]
fn refuses_absent_and_nonfolder_targets() {
	let base = fixture_base();
	let missing = project(
		r#"
[[finder.folders]]
path = "/Missing"
"#,
	);
	assert!(matches!(
		build(&missing, base.path()),
		Err(Error::MissingFolder { folder }) if folder == "/Missing"
	));
	let link = project(
		r#"
[[contents]]
type = "symlink"
destination = "/Link"
target = "/Applications"
[[finder.folders]]
path = "/link"
"#,
	);
	assert!(matches!(
		build(&link, base.path()),
		Err(Error::NotDirectory { folder }) if folder == "/link"
	));
}

#[test]
fn refuses_case_equivalent_generated_store_collisions() {
	let base = fixture_base();
	let config = project(
		r#"
[[contents]]
type = "directory"
destination = "/Docs/.ds_store"
[[finder.folders]]
path = "/Docs"
"#,
	);
	assert!(matches!(
		build(&config, base.path()),
		Err(Error::StoreCollision { path }) if path.as_str() == "/Docs/.ds_store"
	));
}

#[test]
fn enforces_combined_entry_and_metadata_limits() {
	let base = fixture_base();
	let entry_limited = project(
		r#"
[[contents]]
type = "directory"
destination = "/Docs"
[performance]
max_entries = 1
"#,
	);
	assert!(matches!(
		build(&entry_limited, base.path()),
		Err(Error::EntryBudget)
	));
	let byte_limited = project(
		r#"
[performance]
max_metadata_bytes = "1KiB"
"#,
	);
	assert!(matches!(
		build(&byte_limited, base.path()),
		Err(Error::MetadataBudget)
	));
}

#[test]
fn skips_unrequested_stores_and_respects_cancellation() {
	let base = fixture_base();
	let config = project("");
	let manifest = discover(&config, base.path(), Limits::default()).unwrap();
	let no_store = configured_profile(&config, Availability::NotCompiled);
	assert!(generate(&config, &manifest, &no_store).unwrap().is_empty());

	let cancelled = configured_profile(&config, Availability::Available);
	cancelled.write_options().cancellation.cancel();
	assert!(matches!(
		generate(&config, &manifest, &cancelled),
		Err(Error::Cancelled)
	));
}
