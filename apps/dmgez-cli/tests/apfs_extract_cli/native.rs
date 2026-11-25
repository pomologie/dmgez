use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Output};

use serde_json::{Value, json};

use super::extract;

#[path = "../support/native_apfs.rs"]
mod fixture;

#[path = "../support/native_apfs_mutations.rs"]
mod mutations;

use fixture::{
	HARD_LINK_CHECKPOINT_XID, HARD_LINK_FIXTURE, HARD_LINK_VOLUME_OID, HARD_LINK_VOLUME_UUID,
	NATIVE_BLOCK_BYTES, NATIVE_VOLUME_BYTES, SPARSE_FIXTURE, materialize,
};
use mutations::{
	INCOMPLETE_CHECKPOINT_XID, advertise_incomplete_checkpoint, edit_object, replace_field,
};

// Test input and byte patterns come from the sibling native fixture ledgers.
const DATA_PATTERN: &[u8] = b"APFS shared data fork\n";
const DATA_REPETITIONS: usize = 250;
const RESOURCE_PATTERN: &[u8] = b"APFS shared resource fork\n";
const RESOURCE_REPETITIONS: usize = 200;
const DOCS: &str = "/Docs";
const DOCS_ALIAS: &str = "/Docs/alias.bin";
const PRIMARY_LINK: &str = "/primary.bin";
const UNICODE_ALIAS: &str = "/More/Écho.bin";
const SPARSE_BYTES: usize = 786_469;
const ALL_HOLE_BYTES: usize = 12_317;
const SPARSE_WRITES: [(usize, usize, usize, usize); 2] =
	[(131_072, 17, 23, 251), (524_288, 29, 67, 253)];
const OWNER_READ_WRITE: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;
const PERMISSION_MASK: u32 = 0o777;

#[test]
fn exact_raw_forks_keep_the_volume_and_checkpoint_identity() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("native-links.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	let original = fs::metadata(&image).unwrap();

	for (path, selector, fork, kind, expected) in [
		(
			DOCS_ALIAS,
			HARD_LINK_VOLUME_UUID,
			"data",
			"data_fork",
			DATA_PATTERN.repeat(DATA_REPETITIONS),
		),
		(
			UNICODE_ALIAS,
			HARD_LINK_VOLUME_OID,
			"resource",
			"resource_fork",
			RESOURCE_PATTERN.repeat(RESOURCE_REPETITIONS),
		),
	] {
		let output = root.join(format!("{fork}.bin"));
		let flags = [
			"--raw",
			"--volume",
			selector,
			"--xid",
			HARD_LINK_CHECKPOINT_XID,
			"--fork",
			fork,
			"--json",
		];
		let dry = report(
			extract(
				&image,
				path,
				&output,
				&[flags.as_slice(), &["--dry-run"]].concat(),
			),
			"planned",
		);
		assert_hard_link_volume(&dry);
		assert_eq!(dry["plan"]["selection"], kind);
		assert_eq!(dry["plan"]["files"], 1);
		assert_eq!(dry["plan"]["bytes"], expected.len());
		assert!(dry["progress"].is_null());
		assert!(!output.exists());

		let published = report(extract(&image, path, &output, &flags), "complete");
		assert_hard_link_volume(&published);
		assert_eq!(published["plan"], dry["plan"]);
		assert_eq!(published["progress"]["files_published"], 1);
		assert_eq!(published["progress"]["bytes_published"], expected.len());
		assert_eq!(fs::read(&output).unwrap(), expected);
		assert_eq!(permissions(&output), OWNER_READ_WRITE);

		let refused = report(extract(&image, path, &output, &flags), "failed");
		assert!(refused["progress"].is_null());
		assert_eq!(fs::read(&output).unwrap(), expected);
	}

	let after = fs::metadata(&image).unwrap();
	assert_eq!(after.len(), original.len());
	assert_eq!(after.modified().unwrap(), original.modified().unwrap());
}

#[test]
fn portable_tree_projects_data_and_refuses_unicode_child_names() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("native-links.raw");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	let output = root.join("tree");
	let tree = ["--raw", "--data-only", "--json"];
	let short = (DATA_PATTERN.len() * DATA_REPETITIONS - 1).to_string();
	let over_budget = [tree.as_slice(), &["--max-bytes", &short]].concat();

	for (path, flags, diagnostic) in [
		(DOCS, over_budget.as_slice(), "total byte budget"),
		("/More", tree.as_slice(), "encoding"),
	] {
		let result = extract(&image, path, &output, flags);
		assert_failure(&result, diagnostic);
		assert!(!output.exists());
	}

	let planned = report(
		extract(
			&image,
			DOCS,
			&output,
			&[tree.as_slice(), &["--dry-run"]].concat(),
		),
		"planned",
	);
	assert_eq!(planned["plan"]["selection"], "data_tree");
	assert_eq!(planned["plan"]["directories"], 1);
	assert_eq!(planned["plan"]["hard_links_materialized"], 1);
	assert!(
		planned["plan"]["omitted"]
			.as_array()
			.unwrap()
			.contains(&json!("hard_link_topology"))
	);
	assert!(!output.exists());

	let complete = report(extract(&image, DOCS, &output, &tree), "complete");
	assert_eq!(complete["plan"], planned["plan"]);
	assert_eq!(complete["progress"]["directories_created"], 1);
	assert_eq!(complete["progress"]["files_published"], 1);
	assert_eq!(permissions(&output), OWNER_ONLY_DIRECTORY);
	let alias = output.join("alias.bin");
	assert_eq!(
		fs::read(&alias).unwrap(),
		DATA_PATTERN.repeat(DATA_REPETITIONS)
	);
	assert_eq!(permissions(&alias), OWNER_READ_WRITE);
	assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
}

#[test]
fn sparse_tree_publishes_the_exact_logical_bytes() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("sparse.raw");
	let output = root.join("tree");
	materialize(&image, SPARSE_FIXTURE, &[]);

	let result = report(
		extract(&image, "/", &output, &["--raw", "--data-only", "--json"]),
		"complete",
	);
	assert_eq!(result["progress"]["files_published"], 2);
	assert_eq!(
		result["progress"]["bytes_published"],
		SPARSE_BYTES + ALL_HOLE_BYTES
	);
	let mut expected = vec![0; SPARSE_BYTES];
	for (offset, factor, addend, modulus) in SPARSE_WRITES {
		for (index, byte) in expected[offset..][..NATIVE_BLOCK_BYTES]
			.iter_mut()
			.enumerate()
		{
			*byte = u8::try_from((index * factor + addend) % modulus).unwrap();
		}
	}
	assert_eq!(fs::read(output.join("sparse.bin")).unwrap(), expected);
	assert_eq!(
		fs::read(output.join("all-hole.bin")).unwrap(),
		vec![0; ALL_HOLE_BYTES]
	);
	assert_eq!(fs::read_dir(&output).unwrap().count(), 2);

	let absent = root.join("absent-fork");
	let refused = extract(
		&image,
		"/all-hole.bin",
		&absent,
		&["--raw", "--fork", "resource", "--json"],
	);
	assert_failure(&refused, "fork is absent");
	assert!(!absent.exists());
}

#[test]
fn requires_an_admitted_partition_volume_and_checkpoint() {
	const SECOND_PARTITION: &str = "2";
	const UNKNOWN_SELECTOR: &str = "999999";
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let raw = root.join("mbr.raw");
	let output = root.join("file");
	materialize(&raw, HARD_LINK_FIXTURE, &two_data_partitions());

	for options in [
		[].as_slice(),
		&["--partition", "1"],
		&[
			"--partition",
			SECOND_PARTITION,
			"--volume",
			UNKNOWN_SELECTOR,
		],
		&["--partition", SECOND_PARTITION, "--xid", UNKNOWN_SELECTOR],
	] {
		let flags = [&["--raw", "--fork", "data", "--json"][..], options].concat();
		let refused = report(extract(&raw, PRIMARY_LINK, &output, &flags), "failed");
		assert!(refused["progress"].is_null());
		assert!(!output.exists());
	}
	let complete = report(
		extract(
			&raw,
			PRIMARY_LINK,
			&output,
			&[
				"--raw",
				"--partition",
				SECOND_PARTITION,
				"--fork",
				"data",
				"--json",
			],
		),
		"complete",
	);
	assert_hard_link_volume(&complete);
	assert_eq!(
		fs::read(&output).unwrap(),
		DATA_PATTERN.repeat(DATA_REPETITIONS)
	);

	#[cfg(feature = "build")]
	{
		let image = root.join("mbr.dmg");
		let wrapped = super::Command::new(super::BINARY)
			.arg("wrap-raw")
			.arg(&raw)
			.args(["--format", "udzo", "-o"])
			.arg(&image)
			.output()
			.unwrap();
		assert!(wrapped.status.success(), "{wrapped:?}");
		let fork = root.join("wrapped-fork");
		let checked = report(
			extract(
				&image,
				DOCS_ALIAS,
				&fork,
				&[
					"--partition",
					SECOND_PARTITION,
					"--fork",
					"resource",
					"--json",
				],
			),
			"complete",
		);
		assert_hard_link_volume(&checked);
		assert_eq!(
			fs::read(&fork).unwrap(),
			RESOURCE_PATTERN.repeat(RESOURCE_REPETITIONS)
		);
	}
}

#[test]
fn checkpoint_recovery_requires_explicit_selection_before_publication() {
	const DATA_FORK: &[&str] = &["--raw", "--fork", "data", "--json"];
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("incomplete-checkpoint.raw");
	let output = root.join("file");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	advertise_incomplete_checkpoint(&image);
	let incomplete = INCOMPLETE_CHECKPOINT_XID.to_string();

	for options in [[].as_slice(), &["--dry-run"], &["--xid", &incomplete]] {
		let failed = extract(
			&image,
			PRIMARY_LINK,
			&output,
			&[DATA_FORK, options].concat(),
		);
		assert_failure(&failed, "checkpoint");
		assert!(report(failed, "failed")["progress"].is_null());
		assert!(!output.exists());
	}
	let passed = report(
		extract(
			&image,
			PRIMARY_LINK,
			&output,
			&[DATA_FORK, &["--xid", HARD_LINK_CHECKPOINT_XID]].concat(),
		),
		"complete",
	);
	assert_hard_link_volume(&passed);
	assert_eq!(
		fs::read(&output).unwrap(),
		DATA_PATTERN.repeat(DATA_REPETITIONS)
	);
}

#[test]
fn checks_quarantine_on_the_selected_alias_path_and_shared_inode() {
	const DOCS_PROVENANCE_NAME_OFFSET: usize = 435;
	const FILE_PROVENANCE_NAME_OFFSET: usize = 499;
	const RESOURCE: &[&str] = &["--raw", "--fork", "resource", "--json"];
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("quarantine.raw");
	let refused = root.join("refused");
	materialize(&image, HARD_LINK_FIXTURE, &[]);

	mark_native_attribute(&image, DOCS_PROVENANCE_NAME_OFFSET);
	assert_failure(
		&extract(&image, DOCS_ALIAS, &refused, RESOURCE),
		"quarantined",
	);
	assert_failure(
		&extract(
			&image,
			DOCS,
			&refused,
			&["--raw", "--data-only", "--dry-run", "--json"],
		),
		"quarantined",
	);
	assert!(!refused.exists());

	let other_link = root.join("other-link");
	report(
		extract(&image, PRIMARY_LINK, &other_link, RESOURCE),
		"complete",
	);
	assert_eq!(
		fs::read(&other_link).unwrap(),
		RESOURCE_PATTERN.repeat(RESOURCE_REPETITIONS)
	);

	let marked_shared = root.join("shared-inode-marked.raw");
	materialize(&marked_shared, HARD_LINK_FIXTURE, &[]);
	mark_native_attribute(&marked_shared, FILE_PROVENANCE_NAME_OFFSET);
	for path in [PRIMARY_LINK, DOCS_ALIAS, UNICODE_ALIAS] {
		assert_failure(
			&extract(&marked_shared, path, &refused, RESOURCE),
			"quarantined",
		);
		assert!(!refused.exists());
	}
	let marked_member = extract(
		&marked_shared,
		DOCS,
		&refused,
		&["--raw", "--data-only", "--json"],
	);
	assert_failure(&marked_member, "quarantined");
	assert!(report(marked_member, "failed")["progress"].is_null());
	assert!(!refused.exists());
}

#[test]
fn malformed_ancestor_attribute_refuses_projection_before_publication() {
	const DOCS_PROVENANCE_LENGTH_OFFSET: usize = 433;
	const STORED_NAME_BYTES: u16 = 21;
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	let image = root.join("bad-ancestor.raw");
	let output = root.join("refused");
	materialize(&image, HARD_LINK_FIXTURE, &[]);
	edit_object(&image, HARD_LINK_FSROOT, |block| {
		replace_field(
			block,
			DOCS_PROVENANCE_LENGTH_OFFSET,
			STORED_NAME_BYTES.to_le_bytes(),
			0_u16.to_le_bytes(),
		);
	});

	let failed = extract(
		&image,
		DOCS_ALIAS,
		&output,
		&["--raw", "--fork", "data", "--json"],
	);
	assert_failure(&failed, "quarantine");
	let failed = report(failed, "failed");
	assert!(failed["plan"].is_null());
	assert!(failed["progress"].is_null());
	assert!(!output.exists());
}

const HARD_LINK_FSROOT: u64 = 206;

fn mark_native_attribute(image: &Path, name_offset: usize) {
	const PROVENANCE: [u8; 21] = *b"com.apple.provenance\0";
	const QUARANTINE: [u8; 21] = *b"com.apple.quarantine\0";
	edit_object(image, HARD_LINK_FSROOT, |block| {
		replace_field(block, name_offset, PROVENANCE, QUARANTINE);
	});
}

fn two_data_partitions() -> Vec<u8> {
	const SECTOR_BYTES: usize = 512;
	const TABLE_OFFSET: usize = 446;
	const ENTRY_BYTES: usize = 16;
	const TYPE_FIELD: usize = 4;
	const FIRST_SECTOR_FIELD: usize = 8;
	const SECTOR_COUNT_FIELD: usize = 12;
	const SIGNATURE_OFFSET: usize = 510;
	const SIGNATURE: [u8; 2] = [0x55, 0xaa];
	const GENERIC_DATA_PARTITION: u8 = 0xda;
	const OTHER_FIRST_LBA: u32 = 1;
	const APFS_FIRST_LBA: u32 = 2048;
	let apfs_sectors = u32::try_from(NATIVE_VOLUME_BYTES / SECTOR_BYTES as u64).unwrap();
	let mut prefix = vec![0; APFS_FIRST_LBA as usize * SECTOR_BYTES];
	for (index, (start, count)) in [(OTHER_FIRST_LBA, 1_u32), (APFS_FIRST_LBA, apfs_sectors)]
		.into_iter()
		.enumerate()
	{
		let entry = &mut prefix[TABLE_OFFSET + index * ENTRY_BYTES..][..ENTRY_BYTES];
		entry[TYPE_FIELD] = GENERIC_DATA_PARTITION;
		entry[FIRST_SECTOR_FIELD..][..size_of::<u32>()].copy_from_slice(&start.to_le_bytes());
		entry[SECTOR_COUNT_FIELD..][..size_of::<u32>()].copy_from_slice(&count.to_le_bytes());
	}
	prefix[SIGNATURE_OFFSET..SECTOR_BYTES].copy_from_slice(&SIGNATURE);
	prefix
}

fn permissions(path: &Path) -> u32 {
	fs::metadata(path).unwrap().permissions().mode() & PERMISSION_MASK
}

fn assert_failure(output: &Output, expected: &str) {
	assert!(!output.status.success(), "{output:?}");
	let diagnostic = String::from_utf8_lossy(&output.stderr);
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

fn assert_hard_link_volume(report: &Value) {
	assert_eq!(
		report["apfs_volume"],
		json!({
			"uuid": HARD_LINK_VOLUME_UUID.to_ascii_lowercase(),
			"object_id": HARD_LINK_VOLUME_OID,
			"checkpoint_xid": HARD_LINK_CHECKPOINT_XID
		})
	);
}

fn report(output: Output, status: &str) -> Value {
	assert_eq!(output.status.success(), status != "failed", "{output:?}");
	let report: Value = serde_json::from_slice(&output.stdout)
		.unwrap_or_else(|error| panic!("{error}: {output:?}"));
	assert_eq!(report["schema_version"], 1);
	assert_eq!(report["operation"], "extract");
	assert_eq!(report["status"], status);
	report
}
