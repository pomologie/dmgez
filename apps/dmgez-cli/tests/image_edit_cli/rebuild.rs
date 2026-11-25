#[cfg(target_os = "macos")]
use std::process::Command;
use std::{fmt::Write as _, fs, path::Path, process::Output, thread};

use dmgez_builder::preflight::{Availability, OutputIntent, PreflightOptions, ReleaseIntent};
use dmgez_config::Project;
use dmgez_hfsplus_read::browse::{Limits, with_catalog, with_view};
use dmgez_udif::{TRAILER_BYTES, reader::Image, validate_unsigned_signature_fields};
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{ForkKind, LookupName, NodeKind},
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{CHANGES, EDITED, ORIGINAL, assert_failure, edit};

const NOOP: &str = "noop.dmg";
const EMPTY_BATCH: &str = "schema_version = 1\nedits = []\n";
const DATA: &str = "original.txt";
const PAYLOAD: &[u8] = b"original file bytes\0\xff\x00\n";
#[cfg(target_os = "macos")]
const RESOURCE: &[u8] = b"the binary resource fork\0\xff\x00\n";
#[cfg(target_os = "macos")]
const ATTRIBUTE: &str = "com.pomologie.test.inline";
#[cfg(target_os = "macos")]
const ATTRIBUTE_VALUE: &str = "inline attribute value";
#[cfg(unix)]
const LINK: &str = "hard-link.txt";
const FOLDER: &str = "old-folder";
const OPAQUE_LAYOUT: &str = "old-folder/.DS_Store";
const EXPECTED_DIGEST: &str = "--expect-image-sha256";
const STALE_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PROJECT_TEXT: &str = r#"
schema_version = 1
[image]
name = "Original"
output = "original.dmg"
format = "udzo"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_finder_info = false
[[contents]]
source = "original.txt"
destination = "/Original.txt"
[[contents]]
source = "old-folder"
destination = "/Remove"
"#;
#[cfg(unix)]
const LINK_CONTENT: &str = r#"
[[contents]]
source = "hard-link.txt"
destination = "/Alias.txt"
"#;
const BATCH: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Original.txt"
destination = "/Renamed.txt"
[[edits]]
operation = "remove"
target = "/Remove"
[[edits]]
operation = "add_directory"
destination = "/Added"
[[edits]]
operation = "add_symlink"
destination = "/NewLink"
target = "Renamed.txt"
[[edits]]
operation = "set_image_name"
name = "Edited"
"#;

fn build(root: &Path, uses_opaque_layout: bool) {
	fs::write(root.join(DATA), PAYLOAD).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		fs::set_permissions(root.join(DATA), fs::Permissions::from_mode(0o751)).unwrap();
	}
	#[cfg(target_os = "macos")]
	{
		fs::write(root.join(DATA).join("..namedfork/rsrc"), RESOURCE).unwrap();
		let xattr = Command::new("/usr/bin/xattr")
			.current_dir(root)
			.args(["-w", ATTRIBUTE, ATTRIBUTE_VALUE, DATA])
			.output()
			.unwrap();
		assert!(xattr.status.success(), "{xattr:?}");
	}
	#[cfg(unix)]
	fs::hard_link(root.join(DATA), root.join(LINK)).unwrap();
	fs::create_dir(root.join(FOLDER)).unwrap();
	if uses_opaque_layout {
		fs::write(root.join(OPAQUE_LAYOUT), b"opaque layout metadata\0").unwrap();
	}
	#[cfg(unix)]
	let project = format!("{PROJECT_TEXT}{LINK_CONTENT}");
	#[cfg(not(unix))]
	let project = PROJECT_TEXT;
	fs::write(root.join(CHANGES), BATCH).unwrap();
	let project = Project::parse(&project).unwrap();
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.finder_layout = Availability::Unavailable;
	let prepared = dmgez_hfsplus::prepare(
		&project,
		&root.canonicalize().unwrap(),
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: thread::available_parallelism().unwrap(),
		},
		Default::default(),
	)
	.unwrap();
	prepared.seal().unwrap().publish(None).unwrap();
}

#[test]
fn copies_the_edited_catalog_forks_and_attributes_into_a_verified_unsigned_image() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	build(&root, false);
	#[cfg(target_os = "macos")]
	{
		let output = Command::new("/usr/bin/codesign")
			.current_dir(&root)
			.args(["--sign", "-", ORIGINAL])
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	let planned = json(edit(&root, EDITED, &["--dry-run", "--json"]));
	assert_eq!(planned["edits"], 5);
	assert_eq!(planned["published"], false);
	assert_eq!(planned["verified"], false);
	assert!(planned["logical_sha256"].is_null());
	assert!(planned["physical_bytes"].is_null());
	assert!(!root.join(EDITED).exists());
	let mut revision = String::new();
	for byte in Sha256::digest(&original) {
		write!(revision, "{byte:02x}").unwrap();
	}
	assert_eq!(planned["image_sha256"], revision);
	#[cfg(target_os = "macos")]
	assert_eq!(planned["source_envelope"], "signed_digests_verified");
	let published = json(edit(&root, EDITED, &[EXPECTED_DIGEST, &revision, "--json"]));
	assert_eq!(published["schema_version"], 1);
	assert_eq!(published["trust"], "unsigned");
	assert_eq!(published["published"], true);
	assert_eq!(published["verified"], true);
	assert_eq!(published["logical_bytes"], planned["logical_bytes"]);
	assert_eq!(published["entries"], planned["entries"]);
	let rebuilt = fs::read(root.join(EDITED)).unwrap();
	assert_eq!(published["physical_bytes"], rebuilt.len() as u64);
	assert!(
		validate_unsigned_signature_fields(rebuilt.last_chunk::<TRAILER_BYTES>().unwrap()).is_ok()
	);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	assert_eq!(fs::read_to_string(root.join(CHANGES)).unwrap(), BATCH);
	let original_metadata = check_entries(&original, "Original.txt", "Original", false);
	assert_eq!(
		check_entries(&rebuilt, "Renamed.txt", "Edited", true),
		original_metadata
	);
	#[cfg(target_os = "macos")]
	{
		let output = Command::new("/usr/bin/hdiutil")
			.current_dir(&root)
			.args(["verify", EDITED])
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
	assert_failure(edit(&root, EDITED, &[]), "destination already exists");
	assert_eq!(fs::read(root.join(EDITED)).unwrap(), rebuilt);
	fs::write(root.join(CHANGES), EMPTY_BATCH).unwrap();
	let report = json(edit(&root, NOOP, &[EXPECTED_DIGEST, &revision, "--json"]));
	assert_eq!(report["source_envelope"], planned["source_envelope"]);
	assert_eq!(report["edits"], 0);
	assert_eq!(report["trust"], "unsigned");
	assert_eq!(report["verified"], true);
	let noop = fs::read(root.join(NOOP)).unwrap();
	assert!(
		validate_unsigned_signature_fields(noop.last_chunk::<TRAILER_BYTES>().unwrap()).is_ok()
	);
	assert_eq!(
		check_entries(&noop, "Original.txt", "Original", false),
		original_metadata
	);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	assert_eq!(fs::read_to_string(root.join(CHANGES)).unwrap(), EMPTY_BATCH);
}

#[derive(Debug, PartialEq)]
struct FileMetadata {
	mode: u16,
	owner: u32,
	group: u32,
	dates: [u32; 5],
	finder_info: [u8; 32],
}

fn check_entries(bytes: &[u8], filename: &str, label: &str, edited: bool) -> FileMetadata {
	let image = Image::open_release(bytes, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		let retained = view
			.lookup(&root, LookupName::Utf8(filename))
			.unwrap()
			.unwrap()
			.node;
		let expected_absent = if edited {
			"Original.txt"
		} else {
			"Renamed.txt"
		};
		assert!(
			view.lookup(&root, LookupName::Utf8(expected_absent))
				.unwrap()
				.is_none()
		);
		let removed = view.lookup(&root, LookupName::Utf8("Remove")).unwrap();
		let added = view.lookup(&root, LookupName::Utf8("Added")).unwrap();
		let link = view.lookup(&root, LookupName::Utf8("NewLink")).unwrap();
		if edited {
			assert!(removed.is_none());
			assert_eq!(added.unwrap().kind, NodeKind::Directory);
			assert_eq!(view.read_link(&link.unwrap().node).unwrap(), b"Renamed.txt");
		} else {
			assert_eq!(removed.unwrap().kind, NodeKind::Directory);
			assert!(added.is_none());
			assert!(link.is_none());
		}
		view.with_fork(&retained, ForkKind::Data, |reader| {
			assert_eq!(reader.len(), PAYLOAD.len() as u64);
			let mut actual = [0; PAYLOAD.len()];
			reader.read_exact_at(0, &mut actual).unwrap();
			assert_eq!(actual, PAYLOAD);
		})
		.unwrap()
		.unwrap();
		#[cfg(target_os = "macos")]
		{
			view.with_fork(&retained, ForkKind::Resource, |reader| {
				assert_eq!(reader.len(), RESOURCE.len() as u64);
				let mut actual = [0; RESOURCE.len()];
				reader.read_exact_at(0, &mut actual).unwrap();
				assert_eq!(actual, RESOURCE);
			})
			.unwrap()
			.unwrap();
			view.with_xattr(&retained, LookupName::Utf8(ATTRIBUTE), |reader| {
				assert_eq!(reader.len(), ATTRIBUTE_VALUE.len() as u64);
				let mut actual = [0; ATTRIBUTE_VALUE.len()];
				reader.read_exact_at(0, &mut actual).unwrap();
				assert_eq!(actual, ATTRIBUTE_VALUE.as_bytes());
			})
			.unwrap()
			.unwrap();
		}
		#[cfg(unix)]
		{
			let alias = view
				.lookup(&root, LookupName::Utf8("Alias.txt"))
				.unwrap()
				.unwrap();
			let shared = view.stat(&retained).unwrap().hard_link.unwrap();
			assert_eq!(shared.count.get(), 2);
			assert_eq!(view.stat(&alias.node).unwrap().hard_link.unwrap(), shared);
		}
	})
	.unwrap();
	let limits = Limits::default();
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		limits,
		|_volume, catalog| {
			let root = catalog.root().unwrap();
			assert_eq!(String::from_utf16(&root.name).unwrap(), label);
			let entry = catalog.lookup(root.metadata.id, filename).unwrap().unwrap();
			let file = catalog
				.regular_file(entry.metadata.id, limits.hard_links)
				.unwrap()
				.unwrap();
			let metadata = file.metadata();
			FileMetadata {
				mode: metadata.permissions.mode,
				owner: metadata.permissions.uid,
				group: metadata.permissions.gid,
				dates: metadata.dates,
				finder_info: metadata.finder_info,
			}
		},
	)
	.unwrap()
}

#[test]
fn rejects_stale_source_and_existing_output_without_replacing_either() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, false);
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	assert_failure(
		edit(root, EDITED, &[EXPECTED_DIGEST, STALE_DIGEST]),
		"differs from the required revision",
	);
	assert!(!root.join(EDITED).exists());
	assert_failure(edit(root, ORIGINAL, &[]), "destination already exists");
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

#[test]
fn opaque_finder_layout_refuses_the_rebuild_before_publication() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	build(root, true);
	assert_failure(
		edit(root, EDITED, &[]),
		"Finder alias or layout requiring reference updates",
	);
	assert!(!root.join(EDITED).exists());
}

fn json(output: Output) -> Value {
	assert!(output.status.success(), "{output:?}");
	serde_json::from_slice(&output.stdout).unwrap()
}
