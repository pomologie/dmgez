use std::fs;

use dmgez_config::project_edit::MAX_EDIT_BATCH_BYTES;
use serde_json::Value;

use super::*;

const ORIGINAL_IMAGE: &str = "original.dmg";
const ORIGINAL_PAYLOAD: &[u8] = b"unchanged source payload";
#[cfg(all(feature = "hfsplus-read", target_os = "macos"))]
const RESOURCE_PAYLOAD: &[u8] = b"original resource fork bytes";
const REPLACEMENT_PAYLOAD: &[u8] = b"replacement content";
const STALE_SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SOURCE: &str = r#"# Preserve this comment and all formatting.
schema_version = 1
[image]
name = "Original"
output = "original.dmg"
format = "udro"
size = "2MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_extended_attributes = false
preserve_finder_info = false
[signing]
enabled = true
[[contents]]
source = "${INPUT}"
destination = "/Original.txt"
[[contents]]
type = "directory"
destination = "/Remove"
[[contents]]
source = "payload.txt"
destination = "/Replaced.txt"
"#;
const EDITS: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Original.txt"
destination = "/Renamed.txt"
[[edits]]
operation = "remove"
target = "/Remove"
[[edits]]
operation = "remove"
target = "/Replaced.txt"
[[edits]]
operation = "add_copy"
source = "replacement.txt"
destination = "/Replaced.txt"
[[edits]]
operation = "add_directory"
destination = "/Added"
[[edits]]
operation = "set_image_name"
name = "Edited"
"#;
const DEFINE: &[&str] = &["--define", "INPUT=payload.txt", "--json"];

fn fixture(root: &Path) -> std::path::PathBuf {
	let project = root.join(PROJECT);
	let config_dir = project.parent().unwrap();
	fs::create_dir(config_dir).unwrap();
	fs::write(&project, SOURCE).unwrap();
	let payload = config_dir.join("payload.txt");
	fs::write(&payload, ORIGINAL_PAYLOAD).unwrap();
	#[cfg(all(feature = "hfsplus-read", target_os = "macos"))]
	fs::write(payload.join("..namedfork/rsrc"), RESOURCE_PAYLOAD).unwrap();
	fs::write(config_dir.join("replacement.txt"), REPLACEMENT_PAYLOAD).unwrap();
	fs::write(root.join(BATCH), EDITS).unwrap();
	config_dir.to_owned()
}

#[test]
fn applies_one_batch_to_a_verified_unsigned_image_and_leaves_its_inputs_untouched() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let config = fixture(&root);
	let original = config.join(ORIGINAL_IMAGE);
	let edited = config.join(EDITED_IMAGE);
	let build = Command::new(BINARY)
		.current_dir(&root)
		.args(["build", PROJECT, "--define", "INPUT=payload.txt"])
		.output()
		.unwrap();
	assert!(build.status.success(), "{build:?}");
	#[cfg(target_os = "macos")]
	{
		let signed = Command::new("/usr/bin/codesign")
			.args(["--sign", "-"])
			.arg(&original)
			.output()
			.unwrap();
		assert!(signed.status.success(), "{signed:?}");
	}
	let original_bytes = fs::read(&original).unwrap();
	let planned = edit(&root, EDITED_IMAGE, &[DEFINE, &["--dry-run"]].concat());
	let planned = report(planned);
	assert_eq!(planned["published"], false);
	assert_eq!(planned["verified"], false);
	assert_eq!(planned["edits"], 6);
	assert!(planned["physical_bytes"].is_null());
	assert!(!edited.exists());
	let expected = planned["project_sha256"].as_str().unwrap();
	let published = report(edit(
		&root,
		EDITED_IMAGE,
		&[DEFINE, &["--expect-project-sha256", expected]].concat(),
	));
	assert_eq!(published["published"], true);
	assert_eq!(published["verified"], true);
	assert_eq!(published["trust"], "unsigned");
	assert_eq!(published["release_stages_deferred"], true);
	assert_eq!(published["logical_bytes"], planned["logical_bytes"]);
	let bytes = fs::read(&edited).unwrap();
	assert_eq!(published["physical_bytes"], bytes.len() as u64);
	let trailer = bytes.last_chunk::<{ dmgez_udif::TRAILER_BYTES }>().unwrap();
	assert!(dmgez_udif::validate_unsigned_signature_fields(trailer).is_ok());
	#[cfg(feature = "hfsplus-read")]
	{
		assert_eq!(
			cat(&edited, "/Renamed.txt", "data").stdout,
			ORIGINAL_PAYLOAD
		);
		assert_eq!(
			cat(&edited, "/Replaced.txt", "data").stdout,
			REPLACEMENT_PAYLOAD
		);
		assert!(!cat(&edited, "/Original.txt", "data").status.success());
		#[cfg(target_os = "macos")]
		assert_eq!(
			cat(&edited, "/Renamed.txt", "resource").stdout,
			RESOURCE_PAYLOAD
		);
		let listing = Command::new(BINARY)
			.arg("ls")
			.arg(&edited)
			.arg("/")
			.output()
			.unwrap();
		assert!(listing.status.success(), "{listing:?}");
		let listing = String::from_utf8(listing.stdout).unwrap();
		assert!(listing.contains("Added"));
		assert!(!listing.contains("Remove"));
	}
	assert_eq!(fs::read(original).unwrap(), original_bytes);
	assert_eq!(fs::read_to_string(root.join(PROJECT)).unwrap(), SOURCE);
	assert_eq!(fs::read_to_string(root.join(BATCH)).unwrap(), EDITS);
	assert!(!edit(&root, EDITED_IMAGE, DEFINE).status.success());
	assert_eq!(fs::read(edited).unwrap(), bytes);
}

#[test]
fn stale_revision_invalid_batch_and_original_destination_leave_no_output() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let config = fixture(root);
	let stale = edit(
		root,
		EDITED_IMAGE,
		&[DEFINE, &["--expect-project-sha256", STALE_SHA256]].concat(),
	);
	assert_error(stale, "does not match the expected revision");
	let original = edit(root, ORIGINAL_IMAGE, DEFINE);
	assert_error(
		original,
		"requires a new destination distinct from the original",
	);
	fs::write(
		root.join(BATCH),
		format!("{EDITS}\n[[edits]]\noperation = 'remove'\ntarget = '/Missing'\n"),
	)
	.unwrap();
	let invalid = edit(root, EDITED_IMAGE, DEFINE);
	assert_error(invalid, "absent before the batch");
	fs::write(root.join(BATCH), vec![b' '; MAX_EDIT_BATCH_BYTES + 1]).unwrap();
	assert_error(
		edit(root, EDITED_IMAGE, DEFINE),
		&format!("{MAX_EDIT_BATCH_BYTES}-byte input limit"),
	);
	assert!(!config.join(EDITED_IMAGE).exists());
	assert_eq!(fs::read_to_string(root.join(PROJECT)).unwrap(), SOURCE);
}

fn report(output: Output) -> Value {
	assert!(output.status.success(), "{output:?}");
	let report: Value = serde_json::from_slice(&output.stdout).unwrap();
	assert_eq!(report["schema_version"], 1);
	report
}

fn assert_error(output: Output, message: &str) {
	assert!(!output.status.success(), "{output:?}");
	assert!(output.stdout.is_empty());
	assert!(
		String::from_utf8_lossy(&output.stderr).contains(message),
		"{output:?}"
	);
}

#[cfg(feature = "hfsplus-read")]
fn cat(path: &Path, entry: &str, fork: &str) -> Output {
	Command::new(BINARY)
		.arg("cat")
		.arg(path)
		.args([entry, "--fork", fork])
		.output()
		.unwrap()
}
