use std::{fmt::Write as _, fs};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{CAPABILITIES, Client, PROJECT_UPDATE, TOOLS_CALL, TOOLS_LIST, VALID_FILE};

const MANIFEST: &str = r#"schema_version = 1

[image]
name = "Bär" # private fixture label
output = "../existing.dmg"

[[contents]]
source = "../unavailable/plug.app"
position = [10, 20]
hidden = true
hide_extension = false

[[contents]]
type = "directory"
destination = "/Discard"
"#;

#[test]
fn returns_exact_revisions_and_utf8_difference_without_modifying_files() {
	let temp = tempfile::tempdir().unwrap();
	let parent = temp.path().canonicalize().unwrap();
	let root = parent.join("projects");
	let image = parent.join("existing.dmg");
	let manifest = root.join(VALID_FILE);
	fs::create_dir(&root).unwrap();
	fs::write(&manifest, MANIFEST).unwrap();
	fs::write(&image, b"original image").unwrap();
	let mut client = Client::start(&root);
	let read = client.read(VALID_FILE);
	assert_eq!(read["revision_sha256"], sha256(MANIFEST.as_bytes()));

	let proposal = client.tool(
		PROJECT_UPDATE,
		arguments(
			VALID_FILE,
			&read["revision_sha256"],
			json!([{"operation":"set_image_name", "name":"Bör"}]),
		),
	);
	let expected = MANIFEST.replace("Bär", "Bör");
	assert_eq!(proposal["schema_version"], 1);
	assert_eq!(
		proposal["outcome"],
		json!({
			"status":"planned",
			"source": {"revision_sha256": read["revision_sha256"], "manifest_bytes": MANIFEST.len(), "content_entries": 2},
			"proposed": {"revision_sha256": sha256(expected.as_bytes()), "manifest_bytes": expected.len(), "content_entries": 2},
			"requested_edits": 1,
			"difference": {"start_byte": MANIFEST.find('ä').unwrap(), "replaced_bytes": 2, "replacement_bytes": 2},
		})
	);
	assert!(!proposal.to_string().contains("private fixture label"));
	assert_eq!(client.read(VALID_FILE), read);

	let no_op = client.tool(
		PROJECT_UPDATE,
		arguments(
			VALID_FILE,
			&read["revision_sha256"],
			json!([{"operation":"set_image_name", "name":"Bär"}]),
		),
	);
	let outcome = &no_op["outcome"];
	assert_eq!(outcome["status"], "planned");
	assert_eq!(outcome["source"], outcome["proposed"]);
	assert_eq!(outcome["difference"], Value::Null);
	assert_eq!(fs::read_to_string(manifest).unwrap(), MANIFEST);
	assert_eq!(fs::read(image).unwrap(), b"original image");
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
	assert_eq!(fs::read_dir(parent).unwrap().count(), 2);
	client.finish();
}

#[test]
fn applies_one_typed_batch_with_optional_layout_changes_and_no_host_access() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let manifest = root.join(VALID_FILE);
	fs::write(&manifest, MANIFEST).unwrap();
	let mut client = Client::start(&root);
	let read = client.read(VALID_FILE);
	let revision = &read["revision_sha256"];

	let layout = client.tool(
		PROJECT_UPDATE,
		arguments(
			VALID_FILE,
			revision,
			json!([{
				"operation":"modify", "target":"/plug.app", "position":null, "hidden":false,
			}]),
		),
	);
	let expected_layout = MANIFEST
		.replace("position = [10, 20]\n", "")
		.replace("hidden = true", "hidden = false");
	assert_eq!(
		layout["outcome"]["proposed"]["revision_sha256"],
		sha256(expected_layout.as_bytes())
	);

	let batch = client.tool(
		PROJECT_UPDATE,
		arguments(VALID_FILE, revision, json!([
			{"operation":"modify", "target":"/plug.app", "destination":"/Plug.app", "position":[12, 34], "hide_extension":null},
			{"operation":"remove", "target":"/Discard"},
			{"operation":"add_directory", "destination":"/Resources"},
			{"operation":"add_symlink", "destination":"/Applications", "target":"/Applications"},
			{"operation":"add_copy", "source":"../unavailable/new.app"},
		])),
	);
	assert_eq!(batch["outcome"]["status"], "planned");
	assert_eq!(batch["outcome"]["requested_edits"], 5);
	assert_eq!(batch["outcome"]["source"]["content_entries"], 2);
	assert_eq!(batch["outcome"]["proposed"]["content_entries"], 4);
	assert_eq!(fs::read_to_string(manifest).unwrap(), MANIFEST);
	assert_eq!(fs::read_dir(root).unwrap().count(), 1);
	client.finish();
}

#[test]
fn requires_current_exact_revision_and_revalidates_the_edited_project() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let manifest = root.join(VALID_FILE);
	fs::write(&manifest, MANIFEST).unwrap();
	let mut client = Client::start(&root);
	let read = client.read(VALID_FILE);
	let revision = &read["revision_sha256"];
	for (edits, expected_issue) in [
		(
			json!([
				{"operation":"add_directory", "destination":"/New"},
				{"operation":"remove", "target":"/New"},
			]),
			json!({"code":"unknown_target", "edit_index":1}),
		),
		(
			json!([
				{"operation":"remove", "target":"/Discard"},
				{"operation":"modify", "target":"/Discard", "hidden":true},
			]),
			json!({"code":"conflicting_edit", "edit_index":1}),
		),
		(
			json!([{"operation":"add_directory", "destination":"/plug.app"}]),
			json!({"code":"invalid_result", "issue":{"code":"invalid_manifest"}}),
		),
	] {
		let report = client.tool(PROJECT_UPDATE, arguments(VALID_FILE, revision, edits));
		assert_eq!(
			report["outcome"],
			json!({"status":"rejected", "issue":expected_issue})
		);
	}

	let changed = format!("{MANIFEST}# file changed\n");
	let edits = json!([{"operation":"set_image_name", "name":"Preview"}]);
	for bytes in [changed.as_bytes(), &[0xff]] {
		fs::write(&manifest, bytes).unwrap();
		let stale = client.tool(
			PROJECT_UPDATE,
			arguments(VALID_FILE, revision, edits.clone()),
		);
		assert_eq!(
			stale["outcome"],
			json!({
				"status":"stale_revision", "current_revision_sha256":sha256(bytes),
			})
		);
		assert_eq!(fs::read(&manifest).unwrap(), bytes);
	}
	let unreadable = client.tool(
		PROJECT_UPDATE,
		arguments(VALID_FILE, &json!(sha256(&[0xff])), edits),
	);
	assert_eq!(
		unreadable["outcome"],
		json!({"status":"unavailable", "issue":{"code":"invalid_utf8"}})
	);
	client.finish();
}

#[test]
fn uses_the_same_manifest_size_limit_for_source_and_proposal() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let file = root.join(VALID_FILE);
	let mut client = Client::start(&root);
	let capabilities = client.tool(CAPABILITIES, json!({}));
	let limit = usize::try_from(capabilities["max_manifest_bytes"].as_u64().unwrap()).unwrap();
	let padding = "x".repeat(limit - MANIFEST.len() - 1);
	let source = format!("{MANIFEST}#{padding}");
	fs::write(&file, &source).unwrap();
	let read = client.read(VALID_FILE);
	assert_eq!(read["manifest_bytes"], limit);
	let request = arguments(
		VALID_FILE,
		&read["revision_sha256"],
		json!([{"operation":"set_image_name", "name":"longer name"}]),
	);
	let draft = client.tool(PROJECT_UPDATE, request.clone());
	assert_eq!(
		draft["outcome"],
		json!({
			"status":"rejected", "issue":{"code":"proposed_manifest_too_large"},
		})
	);
	assert_eq!(fs::read_to_string(&file).unwrap(), source);

	fs::write(&file, format!("{source}\n")).unwrap();
	let oversized = client.tool(PROJECT_UPDATE, request);
	assert_eq!(
		oversized["outcome"],
		json!({
			"status":"unavailable", "issue":{"code":"project_too_large"},
		})
	);
	client.finish();
}

#[test]
fn advertises_and_enforces_bounded_typed_arguments() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	let mut client = Client::start(&root);
	let listing = client.call(TOOLS_LIST, json!({}));
	let tool = listing["result"]["tools"]
		.as_array()
		.unwrap()
		.iter()
		.find(|tool| tool["name"] == PROJECT_UPDATE)
		.unwrap();
	let schema = &tool["inputSchema"];
	let max_edits = client.tool(CAPABILITIES, json!({}))["max_project_edits"]
		.as_u64()
		.unwrap();
	assert_eq!(schema["additionalProperties"], false);
	assert_eq!(
		schema["required"],
		json!(["file_name", "expected_revision_sha256", "edits"])
	);
	assert_eq!(
		schema["properties"]["expected_revision_sha256"]["pattern"],
		"^[0-9a-f]+$"
	);
	assert_eq!(
		schema["properties"]["expected_revision_sha256"]["minLength"],
		64
	);
	assert_eq!(
		schema["properties"]["expected_revision_sha256"]["maxLength"],
		64
	);
	assert_eq!(schema["properties"]["edits"]["minItems"], 1);
	assert_eq!(schema["properties"]["edits"]["maxItems"], max_edits);

	let revision = json!(sha256(MANIFEST.as_bytes()));
	let valid_edit = json!({"operation":"set_image_name", "name":"Preview"});
	let mut write = arguments(VALID_FILE, &revision, json!([valid_edit]));
	write["write"] = true.into();
	for invalid in [
		json!({"file_name":VALID_FILE, "edits":[valid_edit]}),
		write,
		arguments(VALID_FILE, &json!("x".repeat(64)), json!([valid_edit])),
		arguments(VALID_FILE, &revision, json!([])),
		arguments(
			VALID_FILE,
			&revision,
			json!(vec![valid_edit; usize::try_from(max_edits + 1).unwrap()]),
		),
		arguments(
			VALID_FILE,
			&revision,
			json!([{"operation":"set_output_path", "path":"new.dmg"}]),
		),
		arguments(
			VALID_FILE,
			&revision,
			json!([{"operation":"modify", "target":"/plug.app", "position":[1]}]),
		),
		arguments(
			VALID_FILE,
			&revision,
			json!([{"operation":"remove", "target":"/folder/../plug.app"}]),
		),
		arguments(
			VALID_FILE,
			&revision,
			json!([{"operation":"add_directory", "destination":"/New", "target":"unexpected"}]),
		),
	] {
		let response = client.call(
			TOOLS_CALL,
			json!({"name":PROJECT_UPDATE, "arguments":invalid}),
		);
		assert!(
			response.get("error").is_some() || response["result"]["isError"] == true,
			"{response}"
		);
	}
	let mut invalid = arguments(
		VALID_FILE,
		&revision,
		json!([{"operation":"remove", "target":"/Discard"}]),
	);
	let long_key = "client-supplied-private-argument".repeat(500);
	invalid[long_key.as_str()] = true.into();
	let response = client.call(
		TOOLS_CALL,
		json!({"name":PROJECT_UPDATE, "arguments":invalid}),
	);
	assert!(response.to_string().len() < 1024);
	assert!(
		!response
			.to_string()
			.contains("client-supplied-private-argument")
	);
	assert_eq!(fs::read_dir(root).unwrap().count(), 0);
	client.finish();
}

#[test]
fn uses_the_same_direct_file_admission_as_project_read() {
	let temp = tempfile::tempdir().unwrap();
	let parent = temp.path().canonicalize().unwrap();
	let root = parent.join("projects");
	let outside = parent.join("outside.toml");
	fs::create_dir(&root).unwrap();
	fs::write(&outside, MANIFEST).unwrap();
	fs::hard_link(&outside, root.join("hardlink.toml")).unwrap();
	#[cfg(unix)]
	std::os::unix::fs::symlink(&outside, root.join("alias.toml")).unwrap();
	let mut client = Client::start(&root);
	let revision = json!(sha256(MANIFEST.as_bytes()));
	for (file_name, issue) in [
		("../outside.toml", "invalid_file_name"),
		(outside.to_str().unwrap(), "invalid_file_name"),
		("subdir/project.toml", "invalid_file_name"),
		("hardlink.toml", "project_unavailable"),
		("missing.toml", "project_unavailable"),
		#[cfg(unix)]
		("alias.toml", "project_unavailable"),
	] {
		let report = client.tool(
			PROJECT_UPDATE,
			arguments(
				file_name,
				&revision,
				json!([{"operation":"remove", "target":"/Discard"}]),
			),
		);
		assert_eq!(
			report["outcome"],
			json!({"status":"unavailable", "issue":{"code":issue}})
		);
	}
	assert_eq!(fs::read_to_string(outside).unwrap(), MANIFEST);
	client.finish();
}

fn arguments(file_name: &str, revision: &Value, edits: Value) -> Value {
	json!({"file_name":file_name, "expected_revision_sha256":revision, "edits":edits})
}

fn sha256(bytes: &[u8]) -> String {
	let mut hex = String::new();
	for byte in Sha256::digest(bytes) {
		write!(&mut hex, "{byte:02x}").unwrap();
	}
	hex
}
