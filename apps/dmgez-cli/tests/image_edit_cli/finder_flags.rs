use std::{fs, num::NonZeroUsize, ops::Range, path::Path, process::Command};

use dmgez_builder::preflight::{Availability, OutputIntent, PreflightOptions, ReleaseIntent};
use dmgez_config::Project;
use dmgez_hfsplus_read::browse::{Limits, with_catalog, with_view};
use dmgez_udif::reader::Image;
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{ForkKind, LookupName},
};

use super::{BINARY, CHANGES, EDITED, ORIGINAL, assert_failure, edit, rebuild};

const FINDER_INFO_BYTES: usize = 32;
const FLAGS: Range<usize> = 8..10;
const INVISIBLE: u16 = 0x4000;
const EXTENSION_HIDDEN: u16 = 0x0010;
const SYMLINK_TYPE_AND_CREATOR: &[u8; 8] = b"slnkrhap";
const SOURCE_NAME: &str = "Source.txt";
const RENAMED_NAME: &str = "Renamed.txt";
const FOLDER_NAME: &str = "Folder.txt";
const LINK_NAME: &str = "Link.txt";
const KEPT_NAME: &str = "Keep.txt";
const CLEARED: &str = "cleared.dmg";
const SOURCE_PAYLOAD: &[u8] = b"finder flag fixture\n";
const LINK_TARGET: &[u8] = b"/Keep.txt";

type FinderInfo = [u8; FINDER_INFO_BYTES];

const PROJECT_CONTENTS: &str = r#"
[[contents]]
source = "source.txt"
destination = "/Source.txt"
[[contents]]
type = "directory"
destination = "/Folder.txt"
[[contents]]
type = "directory"
destination = "/Keep.txt"
[[contents]]
type = "symlink"
destination = "/Link.txt"
target = "/Keep.txt"
"#;

const SET_FLAGS: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/source.TXT"
destination = "/Renamed.txt"
hidden = true
hide_extension = true
[[edits]]
operation = "modify"
target = "/Folder.txt"
hidden = true
hide_extension = true
[[edits]]
operation = "modify"
target = "/Link.txt"
hidden = true
hide_extension = true
"#;

const CLEAR_FLAGS: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Renamed.txt"
clear_hidden = true
[[edits]]
operation = "modify"
target = "/Folder.txt"
hide_extension = false
[[edits]]
operation = "modify"
target = "/Link.txt"
hidden = false
clear_hide_extension = true
"#;

#[test]
fn patches_finder_flags_in_a_renamed_file_folder_and_symlink() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let (seed, sidecar_config) = seed_finder_info(&root);
	build(&root, PROJECT_CONTENTS, sidecar_config);
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	let before = finder_info(&original, [SOURCE_NAME, FOLDER_NAME, LINK_NAME, KEPT_NAME]);
	assert_eq!(before[0], seed);
	assert!(before[2].starts_with(SYMLINK_TYPE_AND_CREATOR));

	fs::write(root.join(CHANGES), SET_FLAGS).unwrap();
	let report = rebuild::json(edit(&root, EDITED, &["--json"]));
	assert_eq!(report["verified"], true);
	let edited = fs::read(root.join(EDITED)).unwrap();
	let both_flags = INVISIBLE | EXTENSION_HIDDEN;
	assert_eq!(
		finder_info(&edited, [RENAMED_NAME, FOLDER_NAME, LINK_NAME, KEPT_NAME]),
		[
			with_flags(before[0], both_flags),
			with_flags(before[1], both_flags),
			with_flags(before[2], both_flags),
			before[3],
		]
	);
	assert_retained_contents(&edited);

	fs::write(root.join(CHANGES), CLEAR_FLAGS).unwrap();
	let output = Command::new(BINARY)
		.current_dir(&root)
		.args([
			"edit-image",
			EDITED,
			"--changes",
			CHANGES,
			"--output",
			CLEARED,
			"--json",
		])
		.output()
		.unwrap();
	assert_eq!(rebuild::json(output)["verified"], true);
	let cleared = fs::read(root.join(CLEARED)).unwrap();
	assert_eq!(
		finder_info(&cleared, [RENAMED_NAME, FOLDER_NAME, LINK_NAME, KEPT_NAME]),
		[
			with_flags(before[0], EXTENSION_HIDDEN),
			with_flags(before[1], INVISIBLE),
			before[2],
			before[3],
		]
	);
	assert_retained_contents(&cleared);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	assert_eq!(fs::read(root.join(EDITED)).unwrap(), edited);
}

#[cfg(unix)]
#[test]
fn edits_through_hard_link_aliases_update_the_same_inode_in_batch_order() {
	const HARD_LINK_EDITS: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Original.txt"
hidden = true
hide_extension = true
[[edits]]
operation = "modify"
target = "/Alias.txt"
destination = "/Renamed Alias.txt"
hidden = false
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	rebuild::build(&root, false);
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	let stub_info = finder_info(&original, ["Original.txt", "Alias.txt"]);
	fs::write(root.join(CHANGES), HARD_LINK_EDITS).unwrap();
	assert_eq!(
		rebuild::json(edit(&root, EDITED, &["--json"]))["verified"],
		true
	);
	let edited = fs::read(root.join(EDITED)).unwrap();
	let alias_names = ["Original.txt", "Renamed Alias.txt"];
	assert_eq!(finder_info(&edited, alias_names), stub_info);
	let image = Image::open(edited.as_slice(), dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	let limits = Limits::default();
	with_catalog(&mut disk, VolumeId::WholeDisk, limits, |_, catalog| {
		let root = catalog.root().unwrap();
		let [entry, alias] = alias_names.map(|name| {
			let child = catalog.lookup(root.metadata.id, name).unwrap().unwrap();
			catalog
				.regular_file(child.metadata.id, limits.hard_links)
				.unwrap()
				.unwrap()
		});
		assert_eq!(entry.inode_id(), alias.inode_id());
		assert_eq!(entry.hard_link().unwrap().count.get(), 2);
		assert_eq!(
			entry.metadata().finder_info,
			with_flags([0; FINDER_INFO_BYTES], EXTENSION_HIDDEN)
		);
		assert_eq!(alias.metadata().finder_info, entry.metadata().finder_info);
	})
	.unwrap();
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

#[test]
fn rejects_finder_flag_updates_to_signed_bundle_members_and_app_directories() {
	const BUNDLE_CONTENTS: &str = r#"
[[contents]]
source = "source.txt"
destination = "/Renamed Bundle/Contents/_CodeSignature/CodeResources"
[[contents]]
source = "tool.txt"
destination = "/Renamed Bundle/Contents/Tool.txt"
[[contents]]
type = "directory"
destination = "/Installer.app"
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	const TOOL_SOURCE: &str = "tool.txt";
	fs::write(root.join(TOOL_SOURCE), b"bundle member\n").unwrap();
	#[cfg(unix)]
	let contents = {
		const ALIAS_SOURCE: &str = "outside.txt";
		fs::hard_link(root.join(TOOL_SOURCE), root.join(ALIAS_SOURCE)).unwrap();
		format!(
			"{BUNDLE_CONTENTS}\n[[contents]]\nsource = '{ALIAS_SOURCE}'\ndestination = '/Outside.txt'\n"
		)
	};
	#[cfg(unix)]
	build(&root, &contents, "");
	#[cfg(not(unix))]
	build(&root, BUNDLE_CONTENTS, "");
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	const BUNDLE_MEMBER: &str = "/Renamed Bundle/Contents/Tool.txt";
	const APP: &str = "/Installer.app";
	let targets = [(BUNDLE_MEMBER, BUNDLE_MEMBER), (APP, APP)];
	#[cfg(unix)]
	let targets = targets.into_iter().chain([("/Outside.txt", BUNDLE_MEMBER)]);
	for (target, affected) in targets {
		let edits = format!(
			"schema_version = 1\n[[edits]]\noperation = 'modify'\ntarget = '{target}'\nhidden = true\n"
		);
		fs::write(root.join(CHANGES), edits).unwrap();
		let output = edit(&root, EDITED, &[]);
		assert!(
			String::from_utf8_lossy(&output.stderr).contains(affected),
			"{output:?}"
		);
		assert_failure(
			output,
			"Finder flag edit would change signing-sensitive metadata",
		);
		assert!(!root.join(EDITED).exists());
		assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	}
	const APP_NOOP: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Installer.app"
hidden = false
"#;
	fs::write(root.join(CHANGES), APP_NOOP).unwrap();
	assert_eq!(
		rebuild::json(edit(&root, EDITED, &["--json"]))["verified"],
		true
	);
	let edited = fs::read(root.join(EDITED)).unwrap();
	assert_eq!(
		finder_info(&edited, ["Installer.app"]),
		finder_info(&original, ["Installer.app"])
	);
}

fn build(root: &Path, contents: &str, sidecar_config: &str) {
	fs::write(root.join("source.txt"), SOURCE_PAYLOAD).unwrap();
	let project = Project::parse(&format!(
		r#"
schema_version = 1
[image]
name = "Finder Flags"
output = "{ORIGINAL}"
format = "udzo"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
{sidecar_config}
{contents}
"#
	))
	.unwrap();
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.finder_layout = Availability::Unavailable;
	dmgez_hfsplus::prepare(
		&project,
		root,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
		Default::default(),
	)
	.unwrap()
	.seal()
	.unwrap()
	.publish(None)
	.unwrap();
}

#[cfg(unix)]
fn seed_finder_info(root: &Path) -> (FinderInfo, &'static str) {
	const APPLEDOUBLE_MAGIC: u32 = 0x0005_1607;
	const APPLEDOUBLE_V2: u32 = 0x0002_0000;
	const FINDER_INFO_ENTRY_ID: u32 = 9;
	const FINDER_INFO_OFFSET: u32 = 38;
	const FINDER_INFO_LENGTH: u32 = 32;
	const FINDER_INFO_RECORDS: u16 = 1;
	const SIDECAR: &str = "._source.txt";
	const SIDECAR_CONFIG: &str = "sidecar = \"._source.txt\"";
	const KNOWN_INFO: FinderInfo = [
		b'T', b'E', b'X', b'T', b't', b't', b'x', b't', 0x20, 0x00, 0, 3, 0, 4, 0, 0, 0, 0, 0, 0,
		0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7,
	];
	let mut sidecar = Vec::new();
	sidecar.extend_from_slice(&APPLEDOUBLE_MAGIC.to_be_bytes());
	sidecar.extend_from_slice(&APPLEDOUBLE_V2.to_be_bytes());
	sidecar.extend_from_slice(&[0; 16]);
	sidecar.extend_from_slice(&FINDER_INFO_RECORDS.to_be_bytes());
	sidecar.extend_from_slice(&FINDER_INFO_ENTRY_ID.to_be_bytes());
	sidecar.extend_from_slice(&FINDER_INFO_OFFSET.to_be_bytes());
	sidecar.extend_from_slice(&FINDER_INFO_LENGTH.to_be_bytes());
	sidecar.extend_from_slice(&KNOWN_INFO);
	fs::write(root.join(SIDECAR), sidecar).unwrap();
	(KNOWN_INFO, SIDECAR_CONFIG)
}

#[cfg(not(unix))]
fn seed_finder_info(_root: &Path) -> (FinderInfo, &'static str) {
	([0; FINDER_INFO_BYTES], "")
}

fn finder_info<const N: usize>(bytes: &[u8], names: [&str; N]) -> [FinderInfo; N] {
	let image = Image::open(bytes, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|_, catalog| {
			let root = catalog.root().unwrap();
			names.map(|name| {
				catalog
					.lookup(root.metadata.id, name)
					.unwrap()
					.unwrap()
					.metadata
					.finder_info
			})
		},
	)
	.unwrap()
}

fn with_flags(mut original: FinderInfo, flags: u16) -> FinderInfo {
	let original_flags = u16::from_be_bytes(original[FLAGS].try_into().unwrap());
	original[FLAGS].copy_from_slice(&(original_flags | flags).to_be_bytes());
	original
}

fn assert_retained_contents(bytes: &[u8]) {
	let image = Image::open(bytes, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		assert!(
			view.lookup(&root, LookupName::Utf8(SOURCE_NAME))
				.unwrap()
				.is_none()
		);
		let renamed = view
			.lookup(&root, LookupName::Utf8(RENAMED_NAME))
			.unwrap()
			.unwrap();
		view.with_fork(&renamed.node, ForkKind::Data, |reader| {
			assert_eq!(reader.len(), SOURCE_PAYLOAD.len() as u64);
			let mut data = [0; SOURCE_PAYLOAD.len()];
			reader.read_exact_at(0, &mut data).unwrap();
			assert_eq!(data, SOURCE_PAYLOAD);
		})
		.unwrap()
		.unwrap();
		let link = view
			.lookup(&root, LookupName::Utf8(LINK_NAME))
			.unwrap()
			.unwrap();
		assert_eq!(view.read_link(&link.node).unwrap(), LINK_TARGET);
	})
	.unwrap();
}
