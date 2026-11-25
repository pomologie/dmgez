use std::{fmt::Write as _, fs};

use serde_json::json;

use super::{
	BLAKE3_CONTENTS, BLAKE3_EXACT_CONTENTS, CONTENTS_OPTION, DIFFERENT, EQUAL, EXACT_CONTENTS,
	INFER_RENAMES_OPTION, assert_error,
	hfsplus::{HFSPLUS, build},
	report, semantic_command, semantic_diff,
};

const MAX_BYTES_OPTION: &str = "--max-bytes";
const BLAKE3_POLICY: &str = "blake3";
const BLAKE3_EXACT_POLICY: &str = "blake3_exact";
const RENAME_POLICY: &str = "unique_blake3_forks";
const RENAME_CANDIDATES: &str = "rename_candidates";

#[test]
fn hash_comparison_charges_each_pass_and_skips_confirmation_for_mismatches() {
	const PAYLOAD_BYTES: usize = 80 * 1024;
	const SOURCE: &str = "payload.bin";
	const DESTINATION: &str = "Payload.bin";
	const FILL: u8 = 0x6c;
	const REPLACEMENT: u8 = 0xa8;

	let root = tempfile::tempdir().unwrap();
	let source = root.path().join(SOURCE);
	let entries = file_entries(&[(SOURCE, DESTINATION)]);
	let mut payload = vec![FILL; PAYLOAD_BYTES];
	fs::write(&source, &payload).unwrap();
	let left = build(root.path(), "left", &entries);
	*payload.last_mut().unwrap() = REPLACEMENT;
	fs::write(&source, &payload).unwrap();
	let right = build(root.path(), "right", &entries);

	let baseline = report(&semantic_diff(&left, &left, HFSPLUS, &[]), EQUAL);
	let exact = report(
		&semantic_diff(&left, &left, HFSPLUS, &[CONTENTS_OPTION, EXACT_CONTENTS]),
		EQUAL,
	);
	assert_eq!(exact, baseline);
	let paired_bytes: usize = baseline["compared_bytes"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap();
	assert!(paired_bytes >= PAYLOAD_BYTES);
	for (mode, policy, budget) in [
		(BLAKE3_CONTENTS, BLAKE3_POLICY, paired_bytes),
		(BLAKE3_EXACT_CONTENTS, BLAKE3_EXACT_POLICY, 2 * paired_bytes),
	] {
		let budget = budget.to_string();
		let equal = report(
			&semantic_diff(
				&left,
				&left,
				HFSPLUS,
				&[CONTENTS_OPTION, mode, MAX_BYTES_OPTION, &budget],
			),
			EQUAL,
		);
		assert_eq!(equal["policy"]["contents"], policy, "{equal:#}");
		assert_eq!(equal["compared_bytes"], budget, "{equal:#}");
		assert_eq!(equal["changes"], json!([]), "{equal:#}");
	}
	let too_short = (2 * paired_bytes - 1).to_string();
	let exhausted = semantic_diff(
		&left,
		&left,
		HFSPLUS,
		&[
			CONTENTS_OPTION,
			BLAKE3_EXACT_CONTENTS,
			MAX_BYTES_OPTION,
			&too_short,
		],
	);
	assert_error(&exhausted, "byte payload budget");

	let budget = (2 * paired_bytes - PAYLOAD_BYTES).to_string();
	let changed = report(
		&semantic_diff(
			&left,
			&right,
			HFSPLUS,
			&[
				CONTENTS_OPTION,
				BLAKE3_EXACT_CONTENTS,
				MAX_BYTES_OPTION,
				&budget,
			],
		),
		DIFFERENT,
	);
	assert_eq!(changed["compared_bytes"], budget, "{changed:#}");
	assert_eq!(
		changed["changes"],
		json!([{"path": [DESTINATION], "kind": "changed", "fields": ["data_fork"]}]),
		"{changed:#}"
	);
}

#[test]
fn rename_inference_annotates_unique_content_without_replacing_path_changes() {
	const ORIGINAL: &str = "Original report.bin";
	const MOVED: &str = "Moved report\u{202e}.bin";
	const RIGHT_TO_LEFT_OVERRIDE: char = '\u{202e}';
	const ESCAPED_RIGHT_TO_LEFT_OVERRIDE: &str = "\\u202e";
	const UNIQUE_SOURCE: &str = "unique.dat";
	const DUPLICATE_SOURCE: &str = "duplicate.dat";
	const EMPTY_SOURCE: &str = "empty.dat";
	const LINK_SOURCE: &str = "link.dat";
	const LINK_ALIAS: &str = "link-alias.dat";
	const COMMON_LINK: &str = "Common alias.bin";

	let root = tempfile::tempdir().unwrap();
	for (name, bytes) in [
		(UNIQUE_SOURCE, &b"unique rename witness\0\xff\n"[..]),
		(DUPLICATE_SOURCE, &b"ambiguous rename witness\n"[..]),
		(EMPTY_SOURCE, &[][..]),
		(LINK_SOURCE, &b"shared inode witness\n"[..]),
	] {
		fs::write(root.path().join(name), bytes).unwrap();
	}
	fs::hard_link(root.path().join(LINK_SOURCE), root.path().join(LINK_ALIAS)).unwrap();
	let left = build(
		root.path(),
		"left",
		&file_entries(&[
			(UNIQUE_SOURCE, ORIGINAL),
			(DUPLICATE_SOURCE, "Duplicate original.bin"),
			(EMPTY_SOURCE, "Empty original.bin"),
			(LINK_SOURCE, COMMON_LINK),
			(LINK_ALIAS, "Shared original.bin"),
		]),
	);
	let right = build(
		root.path(),
		"right",
		&file_entries(&[
			(UNIQUE_SOURCE, MOVED),
			(DUPLICATE_SOURCE, "Duplicate first.bin"),
			(DUPLICATE_SOURCE, "Duplicate second.bin"),
			(EMPTY_SOURCE, "Empty moved.bin"),
			(LINK_SOURCE, COMMON_LINK),
			(LINK_ALIAS, "Shared moved.bin"),
		]),
	);

	let baseline = report(&semantic_diff(&left, &right, HFSPLUS, &[]), DIFFERENT);
	assert!(baseline.get(RENAME_CANDIDATES).is_none(), "{baseline:#}");
	let inferred = report(
		&semantic_diff(&left, &right, HFSPLUS, &[INFER_RENAMES_OPTION]),
		DIFFERENT,
	);
	assert_eq!(inferred["policy"]["rename_inference"], RENAME_POLICY);
	assert_eq!(
		inferred[RENAME_CANDIDATES],
		json!([{"from": [ORIGINAL], "to": [MOVED]}]),
		"{inferred:#}"
	);
	assert_eq!(inferred["changes"], baseline["changes"], "{inferred:#}");
	let changes = inferred["changes"].as_array().unwrap();
	for (path, kind) in [(ORIGINAL, "removed"), (MOVED, "added")] {
		assert!(changes.contains(&json!({"path": [path], "kind": kind, "fields": []})));
	}

	let text = semantic_command(&left, &right, HFSPLUS)
		.arg(INFER_RENAMES_OPTION)
		.output()
		.unwrap();
	assert_eq!(text.status.code(), Some(DIFFERENT), "{text:?}");
	let stdout = String::from_utf8(text.stdout).unwrap();
	assert!(!stdout.contains(RIGHT_TO_LEFT_OVERRIDE));
	let candidate = stdout
		.lines()
		.find_map(|line| line.strip_prefix("rename_candidate\t"))
		.unwrap();
	let (from, to) = candidate.split_once('\t').unwrap();
	assert!(to.contains(ESCAPED_RIGHT_TO_LEFT_OVERRIDE));
	assert_eq!(
		serde_json::from_str::<[String; 1]>(from).unwrap(),
		[ORIGINAL]
	);
	assert_eq!(serde_json::from_str::<[String; 1]>(to).unwrap(), [MOVED]);

	let budget = baseline["compared_bytes"].as_str().unwrap();
	let exhausted = semantic_diff(
		&left,
		&right,
		HFSPLUS,
		&[INFER_RENAMES_OPTION, MAX_BYTES_OPTION, budget],
	);
	assert_error(&exhausted, "byte payload budget");
}

fn file_entries(files: &[(&str, &str)]) -> String {
	let mut entries = String::new();
	for (source, path) in files {
		writeln!(
			entries,
			"\n[[contents]]\nsource = {source:?}\ndestination = \"/{path}\""
		)
		.unwrap();
	}
	entries
}
