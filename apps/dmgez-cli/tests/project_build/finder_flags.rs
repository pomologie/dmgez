use std::{fs, path::Path, process::Output};

use dmgez_hfsplus_read::browse::{Limits, with_view};
use dmgez_udif::reader::{Image, Limits as UdifLimits};
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{FinderInfo, LookupName, NodeKind},
};

use super::{BINARY, Command, MANIFEST_NAME};

const OUTPUT: &str = "project.dmg";
const PAYLOAD: &str = "payload.txt";
const FINDER_FLAGS_OFFSET: usize = 8;
const FINDER_INVISIBLE: u16 = 0x4000;
const FINDER_HIDE_EXTENSION: u16 = 0x0010;
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Finder Fixture"
format = "udro"
output = "project.dmg"
[metadata]
policy = "report"
"#;

#[test]
fn writes_flags_to_regular_files_directories_and_symlinks() {
	const CONTENTS: &str = r#"
[[contents]]
source = "payload.txt"
destination = "/Document.txt"
hidden = true
[[contents]]
type = "directory"
destination = "/Folder.ext"
hide_extension = true
[[contents]]
type = "directory"
destination = "/.dot-folder"
[[contents]]
type = "symlink"
destination = "/Link.ext"
target = "Document.txt"
hidden = true
hide_extension = true
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	fs::write(root.join(PAYLOAD), b"fixture payload").unwrap();
	let output = build(root, CONTENTS);
	assert!(output.status.success(), "{output:?}");
	check_flags(
		&root.join(OUTPUT),
		&[
			("Document.txt", FINDER_INVISIBLE, NodeKind::RegularFile),
			("Folder.ext", FINDER_HIDE_EXTENSION, NodeKind::Directory),
			(".dot-folder", 0, NodeKind::Directory),
			(
				"Link.ext",
				FINDER_INVISIBLE | FINDER_HIDE_EXTENSION,
				NodeKind::Symlink,
			),
		],
	);
}

#[cfg(unix)]
#[test]
fn merges_compatible_shared_inode_flags_and_refuses_contradictions() {
	const ALIAS: &str = "alias.txt";
	const CONTENTS: &str = r#"
[[contents]]
source = "payload.txt"
destination = "/A.txt"
hidden = true
[[contents]]
source = "alias.txt"
destination = "/B.txt"
hide_extension = true
"#;
	const CONFLICT: &str = "hidden = false\n";

	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	fs::write(root.join(PAYLOAD), b"shared payload").unwrap();
	fs::hard_link(root.join(PAYLOAD), root.join(ALIAS)).unwrap();
	let output = build(root, CONTENTS);
	assert!(output.status.success(), "{output:?}");
	let flags = FINDER_INVISIBLE | FINDER_HIDE_EXTENSION;
	check_flags(
		&root.join(OUTPUT),
		&[
			("A.txt", flags, NodeKind::RegularFile),
			("B.txt", flags, NodeKind::RegularFile),
		],
	);
	let image = fs::read(root.join(OUTPUT)).unwrap();
	let output = build(root, &format!("{CONTENTS}{CONFLICT}"));
	assert_rejected(
		output,
		"hard-link aliases request contradictory Finder flags",
	);
	assert_eq!(fs::read(root.join(OUTPUT)).unwrap(), image);
}

#[test]
fn refuses_to_add_finder_info_to_renamed_signed_bundle_members() {
	const CONTENTS: &str = r#"
[[contents]]
source = "signature-marker"
destination = "/Renamed/Contents/_CodeSignature/CodeResources"
[[contents]]
source = "payload.txt"
destination = "/Renamed/Contents/Document.txt"
hide_extension = true
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	fs::write(root.join(PAYLOAD), b"sealed resource").unwrap();
	fs::write(root.join("signature-marker"), b"synthetic marker").unwrap();
	assert_rejected(
		build(root, CONTENTS),
		"protected application bundle or framework",
	);
	assert!(!root.join(OUTPUT).exists());
}

#[cfg(feature = "project-edit")]
#[test]
fn changes_finder_flags_in_a_project_rebuild() {
	const EDITS: &str = "edits.toml";
	const EDITED: &str = "edited.dmg";
	const CONTENTS: &str = r#"
[[contents]]
source = "payload.txt"
destination = "/Document.txt"
hidden = true
"#;
	const CHANGES: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Document.txt"
clear_hidden = true
hide_extension = true
"#;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	fs::write(root.join(PAYLOAD), b"fixture payload").unwrap();
	let output = build(root, CONTENTS);
	assert!(output.status.success(), "{output:?}");
	let original = root.join(OUTPUT);
	let bytes = fs::read(&original).unwrap();
	fs::write(root.join(EDITS), CHANGES).unwrap();
	let edited = Command::new(BINARY)
		.current_dir(root)
		.args([
			"edit",
			MANIFEST_NAME,
			"--changes",
			EDITS,
			"--output",
			EDITED,
		])
		.output()
		.unwrap();
	assert!(edited.status.success(), "{edited:?}");
	check_flags(
		&root.join(EDITED),
		&[("Document.txt", FINDER_HIDE_EXTENSION, NodeKind::RegularFile)],
	);
	assert_eq!(fs::read(original).unwrap(), bytes);
	assert_eq!(
		fs::read_to_string(root.join(MANIFEST_NAME)).unwrap(),
		format!("{SETTINGS}{CONTENTS}")
	);
}

fn build(root: &Path, contents: &str) -> Output {
	fs::write(root.join(MANIFEST_NAME), format!("{SETTINGS}{contents}")).unwrap();
	Command::new(BINARY)
		.current_dir(root)
		.args(["build", MANIFEST_NAME])
		.output()
		.unwrap()
}

fn check_flags(path: &Path, expected: &[(&str, u16, NodeKind)]) {
	let bytes = fs::read(path).unwrap();
	let source = Image::open(bytes.as_slice(), UdifLimits::default()).unwrap();
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).unwrap();
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		for &(name, expected_flags, kind) in expected {
			let entry = view.lookup(&root, LookupName::Utf8(name)).unwrap().unwrap();
			assert_eq!(entry.kind, kind, "{name}");
			let flags = match view.finder_info(&entry.node).unwrap() {
				FinderInfo::Absent => 0,
				FinderInfo::Present(info) => u16::from_be_bytes(
					info[FINDER_FLAGS_OFFSET..][..size_of::<u16>()]
						.try_into()
						.unwrap(),
				),
				_ => panic!("inconclusive Finder flags for {name}"),
			};
			assert_eq!(flags, expected_flags, "{name}");
		}
	})
	.unwrap();
}

fn assert_rejected(output: Output, diagnostic: &str) {
	assert!(!output.status.success(), "{output:?}");
	let error = String::from_utf8(output.stderr).unwrap();
	assert!(error.contains(diagnostic), "{error}");
}
