use std::{
	fs::{self, File, FileTimes},
	path::Path,
	time::{Duration, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use std::process::Command;

use dmgez_hfsplus::browse::{Limits, with_view};
use dmgez_udif::reader::Image;
use dmgez_vfs::{
	Disk, VolumeId,
	volume::{ForkKind, LookupName, NodeKind},
};
use serde_json::Value;

use super::{
	EDITED, ORIGINAL, assert_failure, edit_from,
	rebuild::{build, json},
};

const HOST_BATCH: &str = "edits/changes.toml";
const HOST_TREE: &str = "edits/uploads";
const HOST_FILE: &str = "edits/uploads/nested/Content.bin";
const HOST_REPLACEMENT: &str = "edits/replacement.bin";
const HOST_PAYLOAD: &[u8] = b"added payload\0\xff";
const REPLACEMENT: &[u8] = b"replacement payload\0\xfe";
#[cfg(target_os = "macos")]
const RESOURCE: &[u8] = b"host resource fork\x00\xff";
#[cfg(target_os = "macos")]
const ATTRIBUTE: &str = "com.pomologie.test.host-copy";
#[cfg(target_os = "macos")]
const ATTRIBUTE_VALUE: &str = "copied attribute";
#[cfg(unix)]
const LINK_TARGET: &str = "../../outside.txt";
const REPORT_POLICY: [&str; 2] = ["--metadata-policy", "report"];
const STALE_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const COPY_AND_REPLACE: &str = r#"
schema_version = 1
[[edits]]
operation = "add_copy"
source = "uploads"
[[edits]]
operation = "remove"
target = "/Original.txt"
[[edits]]
operation = "add_copy"
source = "replacement.bin"
destination = "/Original.txt"
"#;
const COPY_ONE: &str = r#"
schema_version = 1
[[edits]]
operation = "add_copy"
source = "replacement.bin"
destination = "/Replacement.bin"
"#;
const COLLISION: &str = r#"
schema_version = 1
[[edits]]
operation = "add_copy"
source = "replacement.bin"
destination = "/ORIGINAL.TXT"
"#;
const COPY_TREE: &str = r#"
schema_version = 1
[[edits]]
operation = "add_copy"
source = "uploads"
"#;

fn setup(root: &Path, batch: &str) {
	build(root, false);
	fs::create_dir(root.join("edits")).unwrap();
	fs::write(root.join(HOST_BATCH), batch).unwrap();
	fs::write(root.join(HOST_REPLACEMENT), REPLACEMENT).unwrap();
}

#[test]
fn copies_host_tree_and_replaces_an_entry_with_verified_output() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	setup(root, COPY_AND_REPLACE);
	fs::create_dir_all(root.join(HOST_TREE).join("nested")).unwrap();
	let payload_path = root.join(HOST_FILE);
	fs::write(&payload_path, HOST_PAYLOAD).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::{PermissionsExt, symlink};

		fs::set_permissions(&payload_path, fs::Permissions::from_mode(0o751)).unwrap();
		fs::hard_link(&payload_path, root.join(HOST_TREE).join("linked.bin")).unwrap();
		fs::write(root.join("outside.txt"), b"must remain outside the image").unwrap();
		symlink(LINK_TARGET, root.join(HOST_TREE).join("inert-link")).unwrap();
	}
	#[cfg(target_os = "macos")]
	{
		fs::write(payload_path.join("..namedfork/rsrc"), RESOURCE).unwrap();
		let output = Command::new("/usr/bin/xattr")
			.args(["-w", ATTRIBUTE, ATTRIBUTE_VALUE])
			.arg(&payload_path)
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	let planned = json(edit_from(
		root,
		HOST_BATCH,
		EDITED,
		&[REPORT_POLICY.as_slice(), &["--dry-run", "--json"]].concat(),
	));
	assert_eq!(planned["edits"], 3);
	assert_eq!(planned["published"], false);
	assert!(!root.join(EDITED).exists());
	let published = json(edit_from(
		root,
		HOST_BATCH,
		EDITED,
		&[REPORT_POLICY.as_slice(), &["--json"]].concat(),
	));
	assert_eq!(published["output"], EDITED);
	assert_eq!(published["published"], true);
	assert_eq!(published["verified"], true);
	assert_eq!(published["entries"], planned["entries"]);
	assert_eq!(published["image_sha256"], planned["image_sha256"]);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	assert_eq!(
		fs::read_to_string(root.join(HOST_BATCH)).unwrap(),
		COPY_AND_REPLACE
	);
	assert_eq!(fs::read(payload_path).unwrap(), HOST_PAYLOAD);
	assert!(!root.join("edits").join(EDITED).exists());
	let edited_bytes = fs::read(root.join(EDITED)).unwrap();
	check_copy(&edited_bytes);
	assert_failure(
		edit_from(root, HOST_BATCH, EDITED, &REPORT_POLICY),
		"destination already exists",
	);
	assert_eq!(fs::read(root.join(EDITED)).unwrap(), edited_bytes);
}

fn check_copy(edited: &[u8]) {
	let source = Image::open(edited, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).unwrap();
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		let replacement = view
			.lookup(&root, LookupName::Utf8("Original.txt"))
			.unwrap()
			.unwrap();
		view.with_fork(&replacement.node, ForkKind::Data, |reader| {
			assert_eq!(reader.len(), REPLACEMENT.len() as u64);
			let mut bytes = [0; REPLACEMENT.len()];
			reader.read_exact_at(0, &mut bytes).unwrap();
			assert_eq!(bytes, REPLACEMENT);
		})
		.unwrap()
		.unwrap();
		let folder = view
			.lookup(&root, LookupName::Utf8("uploads"))
			.unwrap()
			.unwrap();
		assert_eq!(folder.kind, NodeKind::Directory);
		let nested = view
			.lookup(&folder.node, LookupName::Utf8("nested"))
			.unwrap()
			.unwrap();
		let payload = view
			.lookup(&nested.node, LookupName::Utf8("Content.bin"))
			.unwrap()
			.unwrap();
		view.with_fork(&payload.node, ForkKind::Data, |reader| {
			assert_eq!(reader.len(), HOST_PAYLOAD.len() as u64);
			let mut bytes = [0; HOST_PAYLOAD.len()];
			reader.read_exact_at(0, &mut bytes).unwrap();
			assert_eq!(bytes, HOST_PAYLOAD);
		})
		.unwrap()
		.unwrap();
		#[cfg(unix)]
		{
			let alias = view
				.lookup(&folder.node, LookupName::Utf8("linked.bin"))
				.unwrap()
				.unwrap();
			let metadata = view.stat(&payload.node).unwrap();
			assert_eq!(metadata.mode & 0o777, 0o751);
			let link = metadata.hard_link.unwrap();
			assert_eq!(link.count.get(), 2);
			assert_eq!(view.stat(&alias.node).unwrap().hard_link.unwrap(), link);
			let symlink = view
				.lookup(&folder.node, LookupName::Utf8("inert-link"))
				.unwrap()
				.unwrap();
			assert_eq!(symlink.kind, NodeKind::Symlink);
			assert_eq!(
				view.read_link(&symlink.node).unwrap(),
				LINK_TARGET.as_bytes()
			);
		}
		#[cfg(target_os = "macos")]
		{
			view.with_fork(&payload.node, ForkKind::Resource, |reader| {
				assert_eq!(reader.len(), RESOURCE.len() as u64);
				let mut bytes = [0; RESOURCE.len()];
				reader.read_exact_at(0, &mut bytes).unwrap();
				assert_eq!(bytes, RESOURCE);
			})
			.unwrap()
			.unwrap();
			view.with_xattr(&payload.node, LookupName::Utf8(ATTRIBUTE), |reader| {
				assert_eq!(reader.len(), ATTRIBUTE_VALUE.len() as u64);
				let mut bytes = [0; ATTRIBUTE_VALUE.len()];
				reader.read_exact_at(0, &mut bytes).unwrap();
				assert_eq!(bytes, ATTRIBUTE_VALUE.as_bytes());
			})
			.unwrap()
			.unwrap();
		}
	})
	.unwrap();
}

#[test]
fn strict_host_metadata_refuses_loss_and_report_mode_discloses_it() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	setup(root, COPY_ONE);
	let stamp = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
	File::options()
		.write(true)
		.open(root.join(HOST_REPLACEMENT))
		.unwrap()
		.set_times(FileTimes::new().set_modified(stamp))
		.unwrap();
	assert_failure(
		edit_from(root, HOST_BATCH, EDITED, &["--json"]),
		"required source metadata could not be preserved",
	);
	assert!(!root.join(EDITED).exists());
	let output = edit_from(
		root,
		HOST_BATCH,
		EDITED,
		&[REPORT_POLICY.as_slice(), &["--dry-run", "--json"]].concat(),
	);
	let report = json(output);
	assert!(report["metadata_losses"].as_u64().unwrap() > 0);
	let details = report["metadata_loss_details"].as_array().unwrap();
	assert!(
		details.iter().any(is_modified_timestamp_loss),
		"{details:?}"
	);
	assert_eq!(report["published"], false);
	assert!(!root.join(EDITED).exists());
	let text = edit_from(
		root,
		HOST_BATCH,
		EDITED,
		&[REPORT_POLICY.as_slice(), &["--dry-run"]].concat(),
	);
	assert!(text.status.success(), "{text:?}");
	let text = String::from_utf8(text.stdout).unwrap();
	assert!(text.contains("metadata_losses = "));
	assert!(
		text.contains("Metadata loss at /Replacement.bin (0 other aliases), Timestamp(Modified)")
	);
}

fn is_modified_timestamp_loss(detail: &Value) -> bool {
	detail["path"] == "/Replacement.bin"
		&& detail["other_aliases"] == 0
		&& detail["field"] == "Timestamp(Modified)"
		&& detail["reason"]
			.as_str()
			.unwrap()
			.contains("TimestampPrecision")
}

#[test]
fn rejects_colliding_host_copy_and_checks_expected_image_before_opening_hosts() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	setup(root, COLLISION);
	let original = fs::read(root.join(ORIGINAL)).unwrap();
	assert_failure(
		edit_from(root, HOST_BATCH, EDITED, &REPORT_POLICY),
		"collide under HFS+ name matching",
	);
	assert!(!root.join(EDITED).exists());
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	fs::remove_file(root.join(HOST_REPLACEMENT)).unwrap();
	assert_failure(
		edit_from(
			root,
			HOST_BATCH,
			EDITED,
			&["--expect-image-sha256", STALE_DIGEST],
		),
		"differs from the required revision",
	);
	assert!(!root.join(EDITED).exists());
}

#[test]
fn rejects_an_output_inside_a_copied_host_tree_before_staging() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	setup(root, COPY_TREE);
	let tree = root.join(HOST_TREE);
	fs::create_dir(&tree).unwrap();
	fs::write(tree.join("payload.bin"), HOST_PAYLOAD).unwrap();
	assert_failure(
		edit_from(root, HOST_BATCH, "edits/uploads/edited.dmg", &REPORT_POLICY),
		"output aliases a captured source or is inside a copied directory",
	);
	assert_eq!(tree.read_dir().unwrap().count(), 1);
	assert!(!tree.join(EDITED).exists());
}

#[test]
fn prepared_host_copy_rejects_a_replaced_source_with_identical_bytes() {
	use dmgez_config::{MetadataPolicy, project_edit::parse_edit_batch};
	use dmgez_hfsplus::rebuild::PreparedEdit;

	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let changes_dir = root.join("edits");
	fs::create_dir(&changes_dir).unwrap();
	let source = root.join(HOST_REPLACEMENT);
	fs::write(&source, REPLACEMENT).unwrap();
	let prepared = PreparedEdit::new(
		parse_edit_batch(COPY_ONE).unwrap(),
		&changes_dir,
		&root.join(EDITED),
		MetadataPolicy::Report,
	)
	.unwrap();
	prepared.validate_sources().unwrap();
	fs::rename(&source, changes_dir.join("captured-source.bin")).unwrap();
	fs::write(&source, REPLACEMENT).unwrap();
	assert!(prepared.validate_sources().is_err());
	assert!(!root.join(EDITED).exists());
}
