use std::fs;

use dmgez_udif::TRAILER_BYTES;
use serde_json::json;

use super::{CAPABILITIES, Client, TOOLS_CALL, TOOLS_LIST};

const IMAGE_INSPECT: &str = "image_inspect";
const IMAGE_VERIFY: &str = "image_verify";
const IMAGE_FILE: &str = "fixture.dmg";
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const XML_LENGTH_OFFSET: usize = 224;

#[test]
fn inspects_and_quickly_verifies_only_after_authorizing_a_separate_image_root() {
	let temp = tempfile::tempdir().unwrap();
	let base = temp.path().canonicalize().unwrap();
	let project_root = base.join("projects");
	let image_root = base.join("images");
	fs::create_dir(&project_root).unwrap();
	fs::create_dir(&image_root).unwrap();
	fs::write(image_root.join(IMAGE_FILE), UDZO).unwrap();
	fs::write(project_root.join("project-only.dmg"), UDZO).unwrap();
	let mut project_server = Client::start(&project_root);
	let disabled = project_server.call(
		TOOLS_CALL,
		json!({"name":IMAGE_INSPECT, "arguments":{"file_name":IMAGE_FILE}}),
	);
	assert!(disabled.get("error").is_some() || disabled["result"]["isError"] == true);
	project_server.finish();

	let mut client = Client::start_with_images(&project_root, Some(&image_root));
	assert_eq!(
		client.tool(CAPABILITIES, json!({}))["image_inspection"],
		true
	);
	let listing = client.call(TOOLS_LIST, json!({}));
	let tools = listing["result"]["tools"].as_array().unwrap();
	for name in [IMAGE_INSPECT, IMAGE_VERIFY] {
		let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
		assert_eq!(tool["annotations"]["readOnlyHint"], true);
		assert_eq!(
			tool["inputSchema"]["properties"]["file_name"]["type"],
			"string"
		);
		assert_eq!(tool["outputSchema"]["type"], "object");
	}
	let inspect = client.tool(IMAGE_INSPECT, json!({"file_name":IMAGE_FILE}));
	assert_eq!(inspect["schema_version"], 1);
	assert_eq!(inspect["outcome"]["status"], "inspected");
	let summary = &inspect["outcome"]["summary"];
	assert_eq!(summary["physical_bytes"], UDZO.len());
	assert_eq!(summary["sector_bytes"], dmgez_udif::SECTOR_BYTES);
	assert!(summary["logical_bytes"].as_u64().unwrap() > 0);
	assert!(summary["block_map_count"].as_u64().unwrap() > 0);
	assert_eq!(summary.as_object().unwrap().len(), 7);
	let verify = client.tool(IMAGE_VERIFY, json!({"file_name":IMAGE_FILE}));
	assert_eq!(
		verify["outcome"],
		json!({
			"status":"verified",
			"summary":summary,
			"coverage":{"structure":"passed", "payload":"not_checked", "filesystem":"not_checked", "security":"not_checked"},
		})
	);

	let mut changed_payload = UDZO.to_vec();
	changed_payload[0] ^= 0xff;
	fs::write(image_root.join("payload-changed.dmg"), changed_payload).unwrap();
	assert_eq!(
		client.tool(IMAGE_VERIFY, json!({"file_name":"payload-changed.dmg"}))["outcome"],
		verify["outcome"]
	);
	assert_eq!(
		client.tool(IMAGE_VERIFY, json!({"file_name":"project-only.dmg"}))["outcome"]["issue"]["code"],
		"image_unavailable"
	);
	let full = client.call(
		TOOLS_CALL,
		json!({"name":IMAGE_VERIFY, "arguments":{"file_name":IMAGE_FILE, "mode":"full"}}),
	);
	assert!(full.get("error").is_some() || full["result"]["isError"] == true);
	assert_eq!(fs::read(image_root.join(IMAGE_FILE)).unwrap(), UDZO);
	client.finish();
}

#[test]
fn rejects_unconfined_or_overbudget_images_and_keeps_the_authorized_directory_held() {
	let temp = tempfile::tempdir().unwrap();
	let base = temp.path().canonicalize().unwrap();
	let root = base.join("images");
	let outside = base.join("outside.dmg");
	fs::create_dir(&root).unwrap();
	fs::write(&outside, UDZO).unwrap();
	fs::write(root.join(IMAGE_FILE), UDZO).unwrap();
	fs::write(root.join("bad.dmg"), b"private image fixture sentinel").unwrap();
	fs::create_dir(root.join("dir.dmg")).unwrap();
	#[cfg(unix)]
	std::os::unix::fs::symlink(&outside, root.join("alias.dmg")).unwrap();
	fs::hard_link(&outside, root.join("linked.dmg")).unwrap();
	let mut client = Client::start_with_images(&base, Some(&root));
	let capabilities = client.tool(CAPABILITIES, json!({}));
	let max_metadata =
		usize::try_from(capabilities["max_image_metadata_bytes"].as_u64().unwrap()).unwrap();
	let mut oversized = UDZO.to_vec();
	let mut trailer = oversized.split_off(UDZO.len() - TRAILER_BYTES);
	oversized.resize(oversized.len() + max_metadata, b' ');
	trailer[XML_LENGTH_OFFSET..][..8].copy_from_slice(&(max_metadata as u64).to_be_bytes());
	oversized.extend_from_slice(&trailer);
	fs::write(root.join("large-metadata.dmg"), oversized).unwrap();
	for tool in [IMAGE_INSPECT, IMAGE_VERIFY] {
		for name in [
			"../outside.dmg",
			"./fixture.dmg",
			"sub/fixture.dmg",
			outside.to_str().unwrap(),
			"https://example.invalid/fixture.dmg",
		] {
			assert_eq!(
				client.tool(tool, json!({"file_name":name}))["outcome"]["issue"]["code"],
				"invalid_file_name"
			);
		}
		for name in ["alias.dmg", "linked.dmg", "dir.dmg"] {
			assert_eq!(
				client.tool(tool, json!({"file_name":name}))["outcome"]["issue"]["code"],
				"image_unavailable"
			);
		}
		let malformed = client.tool(tool, json!({"file_name":"bad.dmg"}));
		assert_eq!(malformed["outcome"]["issue"]["code"], "invalid_format");
		assert!(
			!malformed
				.to_string()
				.contains("private image fixture sentinel")
		);
		assert_eq!(
			client.tool(tool, json!({"file_name":"large-metadata.dmg"}))["outcome"]["issue"]["code"],
			"resource_limit"
		);
	}
	fs::rename(&root, base.join("admitted-images")).unwrap();
	fs::create_dir(&root).unwrap();
	fs::write(
		root.join(IMAGE_FILE),
		b"replacement root was not authorized",
	)
	.unwrap();
	assert_eq!(
		client.tool(IMAGE_INSPECT, json!({"file_name":IMAGE_FILE}))["outcome"]["status"],
		"inspected"
	);
	assert_eq!(fs::read(outside).unwrap(), UDZO);
	client.finish();
}
