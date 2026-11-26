use super::*;

const COMMIT: &str = "4bac7c28f04e4e6b9caef98c2be74db3d4501dbc";
const DIGEST: &str = "7a6da9d4f3983895c0d6c56cab933cb8fb5c63a77bd5858cfe992432a3e31100";
const LOG_FILE: &str = "kani.log";
const VALID_LOG: &str = concat!(
	"Checking harness ranges::bounds...\n",
	" ** 0 of 12 failed (2 unreachable)\n",
	" ** 2 of 2 cover properties satisfied\n",
	"VERIFICATION:- SUCCESSFUL\n",
	"Manual Harness Summary:\n",
	"Complete - 1 successfully verified harnesses, 0 failures, 1 total.\n",
);

fn policy() -> (Policy, Registry) {
	let policy = serde_json::from_value(serde_json::json!({
		"milestone": "0.1.0",
		"profiles": {"ranges": {
			"obligations": ["D-V01"],
			"method": "kani",
			"features": ["read"],
			"check": {"kind": "kani", "filter": "bounds", "harnesses": {"bounds": 2}}
		}}
	}))
	.unwrap();
	let registry = serde_json::from_value(serde_json::json!({
		"obligations": [
			{"id": "D-V01", "activation": "0.1.0", "mandatory_when_activated": true, "required_methods": ["kani"]},
			{"id": "D-V02", "activation": "0.2.0", "mandatory_when_activated": true, "required_methods": ["native"]},
			{"id": "D-V03", "activation": "stretch", "mandatory_when_activated": true, "required_methods": ["fuzz"]}
		]
	})).unwrap();
	(policy, registry)
}

fn record(root: &Path, log: &str) -> Record {
	fs::write(root.join(LOG_FILE), log).unwrap();
	serde_json::from_value(serde_json::json!({
		"schema_version": 1, "obligation_id": "D-V01", "method": "kani",
		"status": "pass", "commit": COMMIT, "dirty": false, "source_hash": DIGEST,
		"profile": "ranges", "features": ["read"], "tool_versions": {"kani": "0.68.0"},
		"command": ["cargo", "kani", "--manifest-path", UDIF_MANIFEST, "--no-default-features", "--features", "read",
			"--harness", "bounds", "--output-format", "terse"],
		"bounds": {"operand_bits": 64}, "proof": {"theorem": "bounds"},
		"artifacts": [{"name": LOG_FILE, "sha256": hex(&Sha256::digest(log)), "uri": LOG_FILE}],
		"runner": {"os": "macOS", "arch": "arm64"}, "duration_ms": 500, "exit_code": 0
	})).unwrap()
}

fn check_records(root: &Path, records: Vec<Record>) -> Result<usize> {
	let (policy, registry) = policy();
	validate(&policy, registry, records, COMMIT, DIGEST, root)
}

#[test]
fn accepts_the_required_current_nonvacuous_run() {
	let tmp = tempfile::tempdir().unwrap();
	let root = tmp.path().canonicalize().unwrap();
	assert_eq!(
		check_records(&root, vec![record(&root, VALID_LOG)]).unwrap(),
		1
	);
}

#[test]
fn rejects_missing_duplicate_stale_dirty_and_nonpassing_records() {
	let tmp = tempfile::tempdir().unwrap();
	let root = tmp.path().canonicalize().unwrap();
	assert!(check_records(&root, vec![]).is_err());
	assert!(
		check_records(
			&root,
			vec![record(&root, VALID_LOG), record(&root, VALID_LOG)]
		)
		.is_err()
	);
	for status in ["timeout", "unsupported", "not_run", "fail"] {
		let mut evidence = record(&root, VALID_LOG);
		evidence.status = status.to_owned();
		assert!(check_records(&root, vec![evidence]).is_err(), "{status}");
	}
	let mut evidence = record(&root, VALID_LOG);
	evidence.dirty = true;
	assert!(check_records(&root, vec![evidence]).is_err());
	let mut evidence = record(&root, VALID_LOG);
	evidence.commit.clear();
	assert!(check_records(&root, vec![evidence]).is_err());
}

#[test]
fn rejects_wrong_features_wrong_commands_and_unreachable_proofs() {
	let tmp = tempfile::tempdir().unwrap();
	let root = tmp.path().canonicalize().unwrap();
	let mut evidence = record(&root, VALID_LOG);
	evidence.features.clear();
	assert!(check_records(&root, vec![evidence]).is_err());
	let mut evidence = record(&root, VALID_LOG);
	*evidence
		.command
		.iter_mut()
		.find(|arg| *arg == "read")
		.unwrap() = "write".into();
	assert!(check_records(&root, vec![evidence]).is_err());
	for (original, replaced) in [
		("12 failed (2 unreachable)", "12 failed (12 unreachable)"),
		("2 of 2 cover", "1 of 2 cover"),
		(
			"Checking harness ranges::bounds",
			"Checking harness ranges::other",
		),
	] {
		let modified = VALID_LOG.replace(original, replaced);
		assert!(check_records(&root, vec![record(&root, &modified)]).is_err());
	}
}

#[test]
fn rejects_artifact_mutation_and_out_of_bundle_paths() {
	let tmp = tempfile::tempdir().unwrap();
	let root = tmp.path().canonicalize().unwrap();
	let evidence = record(&root, VALID_LOG);
	fs::write(root.join(LOG_FILE), "tampered summary").unwrap();
	assert!(check_records(&root, vec![evidence]).is_err());
	let mut evidence = record(&root, VALID_LOG);
	evidence.artifacts[0].uri = "../kani.log".into();
	assert!(check_records(&root, vec![evidence]).is_err());
}
