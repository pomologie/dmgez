//! D-BUILD-02 host discovery tests before filesystem-specific planning.

#![cfg(unix)]

use std::{
	ffi::OsString,
	fs,
	io::{ErrorKind, Read},
	os::unix::{ffi::OsStringExt, fs::symlink},
	path::{Path, PathBuf},
	process::Command,
};

use dmgez_builder::manifest::{
	AssetInspector, Budget, EntryKind, Error, Limits, Manifest, PatternList, discover,
	discover_for_inspection, discover_with_cancellation,
};
use dmgez_config::{Payload, Project};

const HEADER: &str = r#"
schema_version = 1
[image]
name = "Manifest"
output = "output.dmg"
"#;
const COPY: &str = r#"
[[contents]]
source = "tree"
destination = "/Package.app"
"#;

fn project(contents: &str) -> Project {
	Project::parse(&format!("{HEADER}{contents}")).unwrap()
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
	let directory = tempfile::tempdir().unwrap();
	// The platform may spell its configured temporary directory through a symlink.
	let base = directory.path().canonicalize().unwrap();
	fs::create_dir(base.join("tree")).unwrap();
	(directory, base)
}

fn file(base: &Path, path: &str, payload: &[u8]) {
	let path = base.join(path);
	fs::create_dir_all(path.parent().unwrap()).unwrap();
	fs::write(path, payload).unwrap();
}

fn paths(manifest: &Manifest) -> Vec<&str> {
	manifest
		.entries()
		.iter()
		.map(|entry| entry.destination.as_str())
		.collect()
}

#[test]
fn collects_dotfiles_sorted_paths_exact_link_bytes_and_stable_file_snapshots() {
	let (_temp, base) = fixture();
	file(&base, "tree/z/data", b"payload");
	file(&base, "tree/.gitignore", b"ignored\n");
	file(&base, "tree/ignored", b"!");
	fs::hard_link(base.join("tree/z/data"), base.join("tree/.same")).unwrap();
	let raw_link = b"z/target-\xff";
	symlink(
		OsString::from_vec(raw_link.to_vec()),
		base.join("tree/link"),
	)
	.unwrap();
	let options = project(&format!(
		r#"{COPY}
[[contents]]
type = "symlink"
destination = "/Applications"
target = "/Applications"
"#
	));

	let manifest = discover(&options, &base, Limits::default()).unwrap();
	assert_eq!(
		paths(&manifest),
		[
			"/Applications",
			"/Package.app",
			"/Package.app/.gitignore",
			"/Package.app/.same",
			"/Package.app/ignored",
			"/Package.app/link",
			"/Package.app/z",
			"/Package.app/z/data",
		]
	);
	assert_eq!(manifest.regular_payload_bytes(), 23);
	assert!(manifest.metadata_bytes() > 0);
	assert!(matches!(
		&manifest.entries()[0].kind,
		EntryKind::Symlink(target) if target == b"/Applications"
	));
	assert!(matches!(
		&manifest.entries()[5].kind,
		EntryKind::Symlink(target) if target == raw_link
	));
	let file = manifest.entries()[7].source.as_ref().unwrap();
	let linked = manifest.entries()[3].source.as_ref().unwrap();
	assert_eq!(file.metadata().identity, linked.metadata().identity);
	assert!(file.metadata().link_count >= 2);
	let mut payload = Vec::new();
	file.open_regular()
		.unwrap()
		.read_to_end(&mut payload)
		.unwrap();
	assert_eq!(payload, b"payload");
	file.validate().unwrap();
}

#[test]
fn filters_normalized_children_with_exclusion_precedence_and_retains_ancestors() {
	let (_temp, base) = fixture();
	file(&base, "tree/hidden/.note.txt", b"keep");
	file(&base, "tree/hidden/remove.txt", b"remove");
	file(&base, "tree/nested/another/data.txt", b"keep");
	file(&base, "tree/nested/another/ignore.bin", b"ignore");
	file(&base, "tree/private/inherited.txt", b"remove");
	fs::create_dir(base.join("tree/empty")).unwrap();
	let options = project(&format!(
		r#"{COPY}
include = ["**/*.txt", "empty"]
exclude = ["private", "**/remove.txt"]
"#
	));
	let manifest = discover(&options, &base, Limits::default()).unwrap();
	assert_eq!(
		paths(&manifest),
		[
			"/Package.app",
			"/Package.app/empty",
			"/Package.app/hidden",
			"/Package.app/hidden/.note.txt",
			"/Package.app/nested",
			"/Package.app/nested/another",
			"/Package.app/nested/another/data.txt",
		]
	);
	assert!(
		manifest
			.entries()
			.iter()
			.all(|entry| entry.source.is_some())
	);
}

#[test]
fn preserves_directory_links_as_leaves_and_never_resolves_source_ancestor_links() {
	let (_temp, base) = fixture();
	file(&base, "outside/secret.txt", b"outside");
	symlink("../outside", base.join("tree/link")).unwrap();
	let manifest = discover(&project(COPY), &base, Limits::default()).unwrap();
	assert_eq!(paths(&manifest), ["/Package.app", "/Package.app/link"]);
	assert!(matches!(
		&manifest.entries()[1].kind,
		EntryKind::Symlink(target) if target == b"../outside"
	));
	let direct_link = project(
		r#"
[[contents]]
source = "tree/link"
"#,
	);
	assert!(matches!(
		&discover(&direct_link, &base, Limits::default()).unwrap().entries()[0].kind,
		EntryKind::Symlink(target) if target == b"../outside"
	));
	let forbidden = project(
		r#"
[[contents]]
source = "tree/link/secret.txt"
"#,
	);
	assert!(matches!(
		discover(&forbidden, &base, Limits::default()),
		Err(Error::Io { .. })
	));
}

#[test]
fn synthesizes_parents_and_merges_a_compatible_explicit_directory() {
	let (_temp, base) = fixture();
	file(&base, "tree/shared/in-tree", b"tree");
	let options = project(&format!(
		r#"
[[contents]]
type = "directory"
destination = "/Package.app/shared"
hidden = true
[[contents]]
type = "directory"
destination = "/Generated/Additional/Directory"
{COPY}
"#
	));
	let manifest = discover(&options, &base, Limits::default()).unwrap();
	assert_eq!(
		paths(&manifest),
		[
			"/Generated",
			"/Generated/Additional",
			"/Generated/Additional/Directory",
			"/Package.app",
			"/Package.app/shared",
			"/Package.app/shared/in-tree",
		]
	);
	assert!(manifest.entries()[0].source.is_none());
	assert!(manifest.entries()[4].source.is_some());
	assert_eq!(manifest.entries()[4].finder.hidden, Some(true));
}

#[test]
fn rejects_duplicates_and_non_directory_ancestors_discovered_on_disk() {
	let (_temp, base) = fixture();
	file(&base, "tree/entry", b"file");
	let duplicate = project(&format!(
		r#"{COPY}
[[contents]]
type = "directory"
destination = "/Package.app/entry"
"#
	));
	let ancestor = project(&format!(
		r#"{COPY}
[[contents]]
type = "directory"
destination = "/Package.app/entry/child"
"#
	));
	assert!(matches!(
		discover(&duplicate, &base, Limits::default()),
		Err(Error::DestinationConflict { .. })
	));
	assert!(matches!(
		discover(&ancestor, &base, Limits::default()),
		Err(Error::ParentConflict { .. })
	));
}

#[test]
fn refuses_changed_file_and_replaced_intermediate_directory() {
	let (_temp, base) = fixture();
	file(&base, "tree/nested/data", b"before");
	let options = project(COPY);
	let first = discover(&options, &base, Limits::default()).unwrap();
	let source = first.entries()[2].source.as_ref().unwrap();
	file(&base, "tree/nested/data", b"changed length");
	assert!(matches!(
		source.open_regular(),
		Err(Error::SourceChanged { .. })
	));
	let second = discover(&options, &base, Limits::default()).unwrap();
	let source = second.entries()[2].source.as_ref().unwrap();
	fs::rename(base.join("tree/nested"), base.join("outside")).unwrap();
	symlink("../outside", base.join("tree/nested")).unwrap();
	assert!(matches!(
		source.open_regular(),
		Err(Error::SourceChanged { .. })
	));
	assert!(matches!(
		source.validate(),
		Err(Error::SourceChanged { .. })
	));
}

#[test]
fn rejects_output_hardlinks_and_output_under_a_copied_directory() {
	let (_temp, base) = fixture();
	file(&base, "tree/in.bin", b"payload");
	fs::hard_link(base.join("tree/in.bin"), base.join("output.dmg")).unwrap();
	let options = project(COPY);
	assert!(matches!(
		discover(&options, &base, Limits::default()),
		Err(Error::OutputSourceAlias { .. })
	));
	let inspected = discover_for_inspection(&options, &base, Limits::default(), || false).unwrap();
	assert_eq!(paths(&inspected), ["/Package.app", "/Package.app/in.bin"]);
	assert!(matches!(
		discover_for_inspection(&options, &base, Limits::default(), || true),
		Err(Error::Cancelled)
	));
	let nested = Project::parse(&format!(
		"{}{COPY}",
		HEADER.replace("output.dmg", "tree/other.dmg"),
	))
	.unwrap();
	assert!(matches!(
		discover(&nested, &base, Limits::default()),
		Err(Error::OutputSourceAlias { .. })
	));
}

#[test]
fn inspection_captures_regular_assets_and_rejects_replaced_links() {
	const ASSET_PATH: &str = "tree/asset.png";
	const MOVED_ASSET: &str = "renamed.png";
	const PAYLOAD: &[u8] = b"image bytes";
	let (_temp, base) = fixture();
	let options = project(&format!(
		r#"
[[contents]]
source = "{ASSET_PATH}"
"#
	));
	let Payload::Copy(copy) = &options.contents[0].payload else {
		panic!("fixture must declare a copy source");
	};
	let inspector = AssetInspector::new(&base).unwrap();
	assert!(matches!(
		inspector.capture_regular(&copy.source),
		Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound
	));
	file(&base, ASSET_PATH, PAYLOAD);
	let asset = inspector.capture_regular(&copy.source).unwrap();
	assert_eq!(asset.metadata().size, PAYLOAD.len() as u64);
	asset.validate().unwrap();
	let original = base.join(ASSET_PATH);
	let moved = original.with_file_name(MOVED_ASSET);
	fs::rename(&original, &moved).unwrap();
	symlink(&moved, &original).unwrap();
	assert!(matches!(asset.validate(), Err(Error::SourceChanged { .. })));
	assert!(matches!(
		inspector.capture_regular(&copy.source),
		Err(Error::RegularSourceRequired { path }) if path == original
	));
}

#[test]
fn checks_visit_entry_depth_memory_and_cancellation_limits() {
	let (_temp, base) = fixture();
	file(&base, "tree/branch/sub/file", b"data");
	let options = project(COPY);
	let defaults = Limits::default();
	let cases = [
		(
			Limits {
				max_visited: 2,
				..defaults
			},
			Budget::Visited,
		),
		(
			Limits {
				max_entries: 3,
				..defaults
			},
			Budget::Entries,
		),
		(
			Limits {
				max_depth: 3,
				..defaults
			},
			Budget::Depth,
		),
		(
			Limits {
				max_metadata_bytes: 1,
				..defaults
			},
			Budget::Metadata,
		),
	];
	for (limits, budget) in cases {
		assert!(
			matches!(discover(&options, &base, limits), Err(Error::Limit(actual)) if actual == budget)
		);
	}
	let mut checks = 0;
	let cancelled = discover_with_cancellation(&options, &base, defaults, || {
		checks += 1;
		checks == 3
	});
	assert!(matches!(cancelled, Err(Error::Cancelled)));
}

#[test]
fn rejects_invalid_globs_and_globs_on_regular_sources() {
	let (_temp, base) = fixture();
	file(&base, "tree/file", b"data");
	let invalid = project(&format!("{COPY}\ninclude = [\"[\"]"));
	assert!(matches!(
		discover(&invalid, &base, Limits::default()),
		Err(Error::InvalidPattern {
			list: PatternList::Include,
			index: 0,
			..
		})
	));
	let regular = project(
		r#"
[[contents]]
source = "tree/file"
include = ["*"]
"#,
	);
	assert!(matches!(
		discover(&regular, &base, Limits::default()),
		Err(Error::PatternsRequireDirectory { .. })
	));
	let non_recursive = project(&format!("{COPY}\nrecursive = false"));
	assert_eq!(
		paths(&discover(&non_recursive, &base, Limits::default()).unwrap()),
		["/Package.app"]
	);
}

#[test]
fn fails_closed_on_a_fifo_without_opening_it() {
	let (_temp, base) = fixture();
	assert!(
		Command::new("mkfifo")
			.arg(base.join("tree/fifo"))
			.status()
			.unwrap()
			.success()
	);
	assert!(matches!(
		discover(&project(COPY), &base, Limits::default()),
		Err(Error::UnsupportedSource { .. })
	));
}
