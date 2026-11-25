use std::{fs, path::Path, process::Command};

use super::{BINARY, MANIFEST_NAME};

const IMAGE_BYTES: usize = 4 << 20;
const SECTOR_BYTES: usize = 512;
const APM_VOLUME_START: usize = 32 << 10;
const GPT_EDGE_BYTES: usize = 20 << 10;
const APM_VOLUME_SLOT: usize = 2;
const GPT_VOLUME_SLOT: usize = 1;
const IMAGE_NAME: &str = "partitioned.dmg";
const RAW_VOLUME: &str = "volume.dmg";
const WHOLE_EXPORT: &str = "whole.raw";
const VOLUME_EXPORT: &str = "volume.raw";
const PAYLOAD: &str = "payload.txt";
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Partitioned Fixture"
output = "partitioned.dmg"
format = "udro"
size = "4MiB"
[[contents]]
source = "payload.txt"
destination = "/Payload.txt"
[metadata]
preserve_timestamps = false
[build]
reproducible = true
source_date_epoch = 1700000000
"#;

#[test]
fn builds_partitioned_project_images_and_exports_the_exact_hfsplus_volume() {
	for format in ["udro", "udzo"] {
		for (scheme, label, start, end, slot) in [
			("apm", "Apm", APM_VOLUME_START, IMAGE_BYTES, APM_VOLUME_SLOT),
			(
				"gpt",
				"Gpt",
				GPT_EDGE_BYTES,
				IMAGE_BYTES - GPT_EDGE_BYTES,
				GPT_VOLUME_SLOT,
			),
		] {
			let temp = tempfile::tempdir().unwrap();
			let root = temp.path();
			write_project(root);
			let map = format!("image.partition={scheme}");
			let codec = format!("image.format={format}");
			let build = ["build", MANIFEST_NAME, "--set", &map, "--set", &codec];
			let mut preview = build.to_vec();
			preview.push("--dry-run");
			let report = cli(root, &preview);
			assert!(report.contains(&format!("partition={label}\n")));
			assert!(report.contains(&format!("logical_bytes={IMAGE_BYTES}\n")));
			assert!(!root.join(IMAGE_NAME).exists());
			cli(root, &build);

			let report = cli(root, &["inspect", IMAGE_NAME, "--partitions", "--json"]);
			let report: serde_json::Value = serde_json::from_str(&report).unwrap();
			assert_eq!(report["disk"]["scheme"], scheme);
			let volume = &report["disk"]["partitions"][slot - 1];
			assert_eq!(volume["id"]["slot"], slot);
			assert_eq!(volume["logical"]["offset_bytes"], start.to_string());
			cli(root, &["unwrap-raw", IMAGE_NAME, "-o", WHOLE_EXPORT]);
			cli(
				root,
				&[
					"unwrap-raw",
					IMAGE_NAME,
					"--partition",
					&slot.to_string(),
					"-o",
					VOLUME_EXPORT,
				],
			);
			let whole = fs::read(root.join(WHOLE_EXPORT)).unwrap();
			assert_eq!(whole.len(), IMAGE_BYTES);
			let volume = fs::read(root.join(VOLUME_EXPORT)).unwrap();
			assert_eq!(volume, whole[start..end]);

			cli(
				root,
				&[
					"build",
					MANIFEST_NAME,
					"--set",
					"image.format=udrw",
					"--set",
					&format!("image.output={RAW_VOLUME}"),
					"--set",
					&format!("image.size={}B", end - start),
				],
			);
			assert_eq!(volume, fs::read(root.join(RAW_VOLUME)).unwrap());
			#[cfg(target_os = "macos")]
			apple_verify(root, scheme, slot, start, end, &whole);
		}
	}
}

#[test]
fn gpt_identity_is_reproducible_and_changes_with_sealed_payload() {
	const REPEAT: &str = "repeat.dmg";
	const CHANGED: &str = "changed.dmg";
	const REPEAT_RAW: &str = "repeat.raw";
	const CHANGED_RAW: &str = "changed.raw";
	const DISK_GUID_OFFSET: usize = SECTOR_BYTES + 56;
	const VOLUME_GUID_OFFSET: usize = 2 * SECTOR_BYTES + 16;
	const GUID_BYTES: usize = 16;
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	write_project(root);
	let build = |output: &str| {
		cli(
			root,
			&[
				"build",
				MANIFEST_NAME,
				"--set",
				"image.partition=gpt",
				"--set",
				&format!("image.output={output}"),
			],
		);
	};
	build(IMAGE_NAME);
	build(REPEAT);
	assert_eq!(
		fs::read(root.join(IMAGE_NAME)).unwrap(),
		fs::read(root.join(REPEAT)).unwrap(),
	);
	fs::write(root.join(PAYLOAD), b"different data").unwrap();
	build(CHANGED);
	cli(root, &["unwrap-raw", REPEAT, "-o", REPEAT_RAW]);
	cli(root, &["unwrap-raw", CHANGED, "-o", CHANGED_RAW]);
	let repeated = fs::read(root.join(REPEAT_RAW)).unwrap();
	let changed = fs::read(root.join(CHANGED_RAW)).unwrap();
	for offset in [DISK_GUID_OFFSET, VOLUME_GUID_OFFSET] {
		assert_ne!(
			repeated[offset..offset + GUID_BYTES],
			changed[offset..offset + GUID_BYTES],
		);
	}
}

fn write_project(root: &Path) {
	fs::write(root.join(MANIFEST_NAME), SETTINGS).unwrap();
	fs::write(root.join(PAYLOAD), b"original bytes").unwrap();
}

fn cli(directory: &Path, arguments: &[&str]) -> String {
	run(BINARY, directory, arguments)
}

fn run(tool: &str, directory: &Path, arguments: &[&str]) -> String {
	let output = Command::new(tool)
		.current_dir(directory)
		.args(arguments)
		.output()
		.unwrap();
	assert!(
		output.status.success(),
		"{tool} {arguments:?}: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	String::from_utf8(output.stdout).unwrap()
}

#[cfg(target_os = "macos")]
fn apple_verify(root: &Path, scheme: &str, slot: usize, start: usize, end: usize, whole: &[u8]) {
	const HDIUTIL: &str = "/usr/bin/hdiutil";
	const HFS_PARTITION_TYPE: &str = "Apple_HFS";
	const APPLE_EXPORT: &str = "apple-udrw.dmg";
	run(HDIUTIL, root, &["verify", "-nocache", "-quiet", IMAGE_NAME]);
	let pmap = run(HDIUTIL, root, &["pmap", IMAGE_NAME]);
	let header = format!("SCHEME: 1 {},", scheme.to_ascii_uppercase());
	assert!(pmap.lines().any(|row| row.starts_with(&header)), "{pmap}");
	let slot = slot.to_string();
	let lba = (start / SECTOR_BYTES).to_string();
	let sectors = ((end - start) / SECTOR_BYTES).to_string();
	let expected = [slot.as_str(), HFS_PARTITION_TYPE, &lba, &sectors];
	assert!(
		pmap.lines()
			.any(|row| row.split_whitespace().take(expected.len()).eq(expected)),
		"{pmap}"
	);
	run(
		HDIUTIL,
		root,
		&[
			"convert",
			IMAGE_NAME,
			"-format",
			"UDRW",
			"-o",
			APPLE_EXPORT,
			"-quiet",
		],
	);
	assert_eq!(whole, fs::read(root.join(APPLE_EXPORT)).unwrap());
}
