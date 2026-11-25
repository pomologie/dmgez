use std::{collections::BTreeMap, fs, path::Path, process::Command};

use dmgez_finder::inspection::{ReadLimits, Record, RecordValue, inspect};
use dmgez_hfsplus_read::browse::{Limits, with_view};
use dmgez_udif::reader::Image;
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{ForkKind, LookupName},
};
use serde_json::Value;

use super::{BINARY, CHANGES, EDITED, ORIGINAL, edit};

const PROJECT: &str = "finder.toml";
const DATA: &str = "payload.txt";
const MOVE_DATA: &str = "move.txt";
const PAYLOAD: &[u8] = b"presented file bytes\0\xff\n";
#[cfg(target_os = "macos")]
const RESOURCE: &[u8] = b"file resource bytes\0\xff\n";
const ROOT_STORE: &str = "/.DS_Store";
const SOURCE_STORE: &str = "/Source/.DS_Store";
const MOVED_STORE: &str = "/Moved Source/.DS_Store";
const TARGET_STORE: &str = "/Target/.DS_Store";
const NEW_STORE: &str = "/New Layout/.DS_Store";

const DEFAULT_LAYOUT: &str = r#"
schema_version = 1
[image]
name = "Default Finder Layout"
output = "original.dmg"
format = "udzo"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_finder_info = false
[[contents]]
source = "payload.txt"
destination = "/Original.txt"
"#;

const DEFAULT_CHANGES: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Original.txt"
destination = "/Renamed.txt"
[[edits]]
operation = "set_image_name"
name = "Renamed Volume"
"#;

const POSITIONED_LAYOUT: &str = r#"
schema_version = 1
[image]
name = "Positioned Finder Layout"
output = "original.dmg"
format = "udzo"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_finder_info = false
[[contents]]
source = "payload.txt"
destination = "/Rename.txt"
position = [101, 102]
[[contents]]
source = "move.txt"
destination = "/Source/Move.txt"
position = [201, 202]
[[contents]]
source = "payload.txt"
destination = "/Target/Clear.txt"
position = [301, 302]
[[contents]]
source = "payload.txt"
destination = "/Target/Set.txt"
[[contents]]
source = "payload.txt"
destination = "/Target/Stay.txt"
position = [501, 502]
[[contents]]
source = "payload.txt"
destination = "/New Layout/First.txt"
[finder]
window_size = [620, 270]
show_status_bar = true
[finder.icon_view]
icon_size = 72
[[finder.folders]]
path = "/Source"
window_size = [400, 300]
[[finder.folders]]
path = "/Target"
window_size = [480, 320]
"#;

const POSITIONED_CHANGES: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/rename.TXT"
destination = "/Renamed.txt"
[[edits]]
operation = "modify"
target = "/Source"
destination = "/Moved Source"
[[edits]]
operation = "modify"
target = "/source/MOVE.txt"
destination = "/Target/Moved.txt"
[[edits]]
operation = "modify"
target = "/Target/Clear.txt"
clear_position = true
[[edits]]
operation = "modify"
target = "/Target/Set.txt"
position = [401, 402]
[[edits]]
operation = "modify"
target = "/New Layout/First.txt"
position = [601, 602]
"#;

#[test]
fn preserves_the_default_finder_store_byte_for_byte_when_it_has_no_changed_references() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let original = build(root, DEFAULT_LAYOUT);
	let before = fork(&original, ROOT_STORE, ForkKind::Data).unwrap();
	let layout = folder_layout(&before);
	assert!(layout.positions.is_empty());
	assert!(!layout.settings.is_empty());

	let edited = apply(root, DEFAULT_CHANGES);
	assert_eq!(fork(&edited, ROOT_STORE, ForkKind::Data).unwrap(), before);
	assert_eq!(
		fork(&edited, "/Renamed.txt", ForkKind::Data).unwrap(),
		PAYLOAD
	);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

#[test]
fn updates_icon_positions_by_original_entry_and_keeps_each_folders_settings() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	fs::write(root.join(MOVE_DATA), PAYLOAD).unwrap();
	#[cfg(target_os = "macos")]
	fs::write(root.join(MOVE_DATA).join("..namedfork/rsrc"), RESOURCE).unwrap();
	let original = build(root, POSITIONED_LAYOUT);
	let root_before = layout_at(&original, ROOT_STORE);
	let source_before = layout_at(&original, SOURCE_STORE);
	let target_before = layout_at(&original, TARGET_STORE);
	assert_positions(&root_before, [("Rename.txt", [101, 102])]);
	assert_positions(&source_before, [("Move.txt", [201, 202])]);
	assert_positions(
		&target_before,
		[("Clear.txt", [301, 302]), ("Stay.txt", [501, 502])],
	);
	assert_ne!(source_before.settings, target_before.settings);
	assert!(fork(&original, NEW_STORE, ForkKind::Data).is_none());
	#[cfg(target_os = "macos")]
	assert_eq!(
		fork(&original, "/Source/Move.txt", ForkKind::Resource).unwrap(),
		RESOURCE
	);

	let edited = apply(root, POSITIONED_CHANGES);
	let root_after = layout_at(&edited, ROOT_STORE);
	let moved_after = layout_at(&edited, MOVED_STORE);
	let target_after = layout_at(&edited, TARGET_STORE);
	assert_eq!(root_before.settings, root_after.settings);
	assert_eq!(source_before.settings, moved_after.settings);
	assert_eq!(target_before.settings, target_after.settings);
	assert_positions(&root_after, [("Renamed.txt", [101, 102])]);
	assert!(moved_after.positions.is_empty());
	assert_positions(
		&target_after,
		[
			("Moved.txt", [201, 202]),
			("Set.txt", [401, 402]),
			("Stay.txt", [501, 502]),
		],
	);
	assert_positions(&layout_at(&edited, NEW_STORE), [("First.txt", [601, 602])]);
	assert!(fork(&edited, "/Moved Source/Move.txt", ForkKind::Data).is_none());
	assert_eq!(
		fork(&edited, "/Target/Moved.txt", ForkKind::Data).unwrap(),
		PAYLOAD
	);
	#[cfg(target_os = "macos")]
	{
		assert_eq!(
			fork(&edited, "/Target/Moved.txt", ForkKind::Resource).unwrap(),
			RESOURCE
		);
		let output = Command::new("/usr/bin/hdiutil")
			.current_dir(root)
			.args(["verify", EDITED])
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

fn build(root: &Path, project: &str) -> Vec<u8> {
	fs::write(root.join(DATA), PAYLOAD).unwrap();
	fs::write(root.join(PROJECT), project).unwrap();
	let image = build_project(root, PROJECT);
	assert_eq!(fs::read_to_string(root.join(PROJECT)).unwrap(), project);
	image
}

pub(super) fn build_project(root: &Path, project: &str) -> Vec<u8> {
	let output = Command::new(BINARY)
		.current_dir(root)
		.args(["build", project])
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
	fs::read(root.join(ORIGINAL)).unwrap()
}

fn apply(root: &Path, changes: &str) -> Vec<u8> {
	fs::write(root.join(CHANGES), changes).unwrap();
	let output = edit(root, EDITED, &["--json"]);
	assert!(output.status.success(), "{output:?}");
	let report: Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["published"], true);
	assert_eq!(report["verified"], true);
	assert_eq!(fs::read_to_string(root.join(CHANGES)).unwrap(), changes);
	fs::read(root.join(EDITED)).unwrap()
}

pub(super) fn fork(image: &[u8], path: &str, kind: ForkKind) -> Option<Vec<u8>> {
	let image = Image::open(image, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let mut node = view.root().unwrap();
		for name in path.strip_prefix('/').unwrap().split('/') {
			node = view.lookup(&node, LookupName::Utf8(name)).unwrap()?.node;
		}
		view.with_fork(&node, kind, |reader| {
			let mut bytes = vec![0; reader.len().try_into().unwrap()];
			reader.read_exact_at(0, &mut bytes).unwrap();
			bytes
		})
		.unwrap()
	})
	.unwrap()
}

struct FolderLayout {
	positions: BTreeMap<String, [u32; 2]>,
	settings: Vec<Record>,
}

fn layout_at(image: &[u8], path: &str) -> FolderLayout {
	folder_layout(&fork(image, path, ForkKind::Data).unwrap())
}

fn folder_layout(bytes: &[u8]) -> FolderLayout {
	let report = inspect(bytes, ReadLimits::default()).unwrap();
	let mut positions = BTreeMap::new();
	let mut settings = Vec::new();
	for record in report.records {
		if let RecordValue::IconPosition { x, y } = record.value {
			assert!(positions.insert(record.name, [x, y]).is_none());
		} else {
			settings.push(record);
		}
	}
	FolderLayout {
		positions,
		settings,
	}
}

fn assert_positions<const N: usize>(layout: &FolderLayout, expected: [(&str, [u32; 2]); N]) {
	let expected = BTreeMap::from(expected.map(|(name, point)| (name.to_owned(), point)));
	assert_eq!(layout.positions, expected);
}
