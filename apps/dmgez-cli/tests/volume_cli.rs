use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");

#[cfg(all(feature = "build", feature = "hfsplus-read"))]
#[path = "volume_cli/lazy.rs"]
mod lazy;

fn invoke(command: &str, image: &Path, options: &[&str]) -> Output {
	Command::new(BINARY)
		.arg(command)
		.arg(image)
		.args(options)
		.output()
		.unwrap()
}

#[cfg(not(feature = "hfsplus-read"))]
#[test]
fn volume_commands_report_the_disabled_backend_before_opening_an_image() {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("absent.dmg");
	for command in ["ls", "tree", "stat", "cat"] {
		let output = invoke(command, &absent, &["/"]);
		assert!(!output.status.success(), "{command}");
		assert!(output.stdout.is_empty(), "{command}");
		assert!(
			String::from_utf8_lossy(&output.stderr).contains("feature not compiled: hfsplus-read"),
			"{command}: {}",
			String::from_utf8_lossy(&output.stderr),
		);
	}
	assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[cfg(feature = "hfsplus-read")]
mod browse {
	use std::{collections::BTreeSet, env, fs, path::PathBuf};

	use serde_json::Value;

	use super::*;

	const FIXTURE_DIRECTORY_ENV: &str = "DMGEZ_HFSPLUS_FIXTURES";
	const FIXTURE_APM: &[u8] =
		include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	pub(super) const FIXTURE_HARD_LINKS: &str = "native-chained-hardlinks.bin";
	const FIXTURE_CATALOG: &str = "native-catalog-volume.bin";
	const FIXTURE_CASE_SENSITIVE: &str = "native-hfsx.bin";
	const CASE_FOLDED_LEDGER: &str = "native-hfsx-case-folded.json";
	pub(super) const HARD_LINK_DATA: &[u8] = b"pomologie linked file\n";
	pub(super) const HARD_LINK_RESOURCE: &[u8] = b"pomologie resource\0fork\n";
	const UNICODE_CATALOG_DATA: &[u8] = b"catalog unicode payload\n";
	const HFS_NATIVE_MODE: u16 = 0o100751;
	const HFS_NATIVE_UID: u32 = 501;
	const HFS_NATIVE_GID: u32 = 20;
	const HFS_NATIVE_LINK_COUNT: u32 = 3;
	const HFS_SHARED_INODE: u64 = 19;
	pub(super) const HARD_LINK_VOLUME_BYTES: usize = 16 * 1024 * 1024;
	const CASE_SENSITIVE_VOLUME_BYTES: usize = 24 * 1024 * 1024;
	const SECTOR_BYTES: usize = 512;
	const MBR_SIGNATURE: [u8; 2] = [0x55, 0xaa];
	const MBR_TABLE: usize = 446;
	const MBR_ENTRY_BYTES: usize = 16;
	const MBR_TYPE_FIELD: usize = 4;
	const MBR_FIRST_SECTOR_FIELD: usize = 8;
	const MBR_SECTORS_FIELD: usize = 12;
	const MBR_HFS_TYPE: u8 = 0xaf;
	const NATIVE_APM_HFS_SLOT: &str = "2";
	const NATIVE_APM_METADATA_SLOT: &str = "1";

	pub(super) fn hfsplus_fixture(name: &str) -> Vec<u8> {
		let path = fixture_path(name);
		fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
	}

	fn fixture_path(name: &str) -> PathBuf {
		let root = env::var_os(FIXTURE_DIRECTORY_ENV).expect("composition supplies HFS+ fixtures");
		PathBuf::from(root).join(name)
	}

	// Sibling fixture ledgers describe the sparse format and the independent
	// filesystem oracles. The CLI receives reconstructed raw disk bytes.
	pub(super) fn materialize_sparse(mut fixture: &[u8], expected_length: usize) -> Vec<u8> {
		fn read_field<const WIDTH: usize>(input: &mut &[u8]) -> [u8; WIDTH] {
			let (value, rest) = input.split_first_chunk().unwrap();
			*input = rest;
			*value
		}

		let len = usize::try_from(u64::from_be_bytes(read_field(&mut fixture))).unwrap();
		assert_eq!(len, expected_length);
		let runs = u16::from_be_bytes(read_field(&mut fixture));
		let mut bytes = vec![0; len];
		for _ in 0..runs {
			let start = usize::try_from(u64::from_be_bytes(read_field(&mut fixture))).unwrap();
			let count = usize::try_from(u32::from_be_bytes(read_field(&mut fixture))).unwrap();
			let (record, rest) = fixture.split_at(count);
			bytes[start..][..count].copy_from_slice(record);
			fixture = rest;
		}
		assert!(fixture.is_empty());
		bytes
	}

	pub(super) fn success(output: Output) -> Vec<u8> {
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr),
		);
		output.stdout
	}

	fn failure(output: Output) -> String {
		assert!(!output.status.success(), "{output:?}");
		assert!(output.stdout.is_empty(), "{output:?}");
		String::from_utf8(output.stderr).unwrap()
	}

	pub(super) fn report(command: &str, image: &Path, path: &str, options: &[&str]) -> Value {
		let args: Vec<_> = [path, "--json"]
			.into_iter()
			.chain(options.iter().copied())
			.collect();
		let value: Value = serde_json::from_slice(&success(invoke(command, image, &args))).unwrap();
		assert_eq!(value["schema_version"], 1);
		assert_eq!(value["operation"], command);
		assert_eq!(value["path"], path);
		value
	}

	fn stored_name(value: &Value) -> String {
		assert_eq!(value["encoding"], "utf16");
		let units: Vec<u16> = serde_json::from_value(value["value"].clone()).unwrap();
		String::from_utf16(&units).unwrap()
	}

	fn entries(value: &Value) -> &[Value] {
		value["entries"].as_array().unwrap()
	}

	fn decimal_id(value: &Value) -> u64 {
		value.as_str().unwrap().parse().unwrap()
	}

	fn names(value: &Value) -> Vec<String> {
		entries(value)
			.iter()
			.map(|entry| stored_name(&entry["name"]))
			.collect()
	}

	#[test]
	fn native_apm_window_is_selected_without_mounting() {
		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("native-apm.dmg");
		fs::write(&image, FIXTURE_APM).unwrap();

		let implicit = report("stat", &image, "/", &[]);
		let explicit = report("stat", &image, "/", &["--partition", NATIVE_APM_HFS_SLOT]);
		assert_eq!(implicit["kind"], "directory");
		assert_eq!(explicit["kind"], "directory");
		assert_eq!(implicit["mode"], explicit["mode"]);
		assert_eq!(
			explicit["volume"],
			serde_json::json!({"kind": "partition", "id": {"scheme": "apm", "slot": 2}}),
		);
		assert_eq!(implicit["volume"], explicit["volume"]);
		failure(invoke(
			"ls",
			&image,
			&["--partition", NATIVE_APM_METADATA_SLOT, "--json"],
		));
		failure(invoke("ls", &image, &["--partition", "99", "--json"]));
		failure(invoke("ls", &image, &["--partition", "0", "--json"]));
		failure(invoke("ls", &image, &["--raw", "--json"]));
		assert_eq!(fs::read(&image).unwrap(), FIXTURE_APM);
		assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
	}

	#[test]
	fn native_hfs_plus_pages_tree_metadata_and_exact_forks() {
		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("hardlinks.raw");
		let bytes =
			materialize_sparse(&hfsplus_fixture(FIXTURE_HARD_LINKS), HARD_LINK_VOLUME_BYTES);
		fs::write(&image, &bytes).unwrap();
		failure(invoke("ls", &image, &["--json"]));

		let first = report("ls", &image, "/", &["--raw", "--limit", "1"]);
		assert_eq!(names(&first), ["four"]);
		assert_eq!(first["next_offset"], 1);
		assert_eq!(first["volume"]["kind"], "whole_disk");
		let middle = report(
			"ls",
			&image,
			"/",
			&["--raw", "--offset", "1", "--limit", "2"],
		);
		assert_eq!(names(&middle), ["nested", "one"]);
		assert_eq!(middle["next_offset"], 3);
		let end = report(
			"ls",
			&image,
			"/",
			&["--raw", "--offset", "3", "--limit", "2"],
		);
		assert_eq!(names(&end), ["two"]);
		assert!(end["next_offset"].is_null());
		assert_eq!(
			names(&report("ls", &image, "/", &["--raw"])),
			["four", "nested", "one", "two"]
		);

		let shallow = report("tree", &image, "/", &["--raw", "--depth", "1"]);
		assert!(!names(&shallow).iter().any(|name| name == "three"));
		let deeper = report("tree", &image, "/", &["--raw", "--depth", "2"]);
		let nested_child = entries(&deeper)
			.iter()
			.find(|entry| stored_name(&entry["name"]) == "three")
			.unwrap();
		assert_eq!(nested_child["kind"], "regular_file");
		assert_eq!(nested_child["depth"], 2);
		assert_eq!(deeper["truncated"], false);
		let capped = report("tree", &image, "/", &["--raw", "--limit", "1"]);
		assert_eq!(entries(&capped).len(), 1);
		assert_eq!(capped["truncated"], true);

		let mut visible_ids = BTreeSet::new();
		for (path, name) in [("/one", "one"), ("/two", "two"), ("/nested/three", "three")] {
			let file = report("stat", &image, path, &["--raw"]);
			assert_eq!(stored_name(&file["name"]), name);
			assert_eq!(file["kind"], "regular_file");
			assert_eq!(file["mode"], HFS_NATIVE_MODE);
			assert_eq!(file["uid"], HFS_NATIVE_UID);
			assert_eq!(file["gid"], HFS_NATIVE_GID);
			assert_eq!(file["hard_link_count"], HFS_NATIVE_LINK_COUNT);
			assert_eq!(decimal_id(&file["hard_link_storage_id"]), HFS_SHARED_INODE);
			assert!(visible_ids.insert(decimal_id(&file["native_id"])));
			assert!(file["link_target"].is_null());
			assert_eq!(
				success(invoke("cat", &image, &[path, "--raw"])),
				HARD_LINK_DATA
			);
			assert_eq!(
				success(invoke(
					"cat",
					&image,
					&[path, "--raw", "--fork", "resource"]
				)),
				HARD_LINK_RESOURCE,
			);
		}
		assert!(
			success(invoke(
				"cat",
				&image,
				&["/four", "--raw", "--fork", "resource"]
			))
			.is_empty()
		);
		failure(invoke("cat", &image, &["/nested", "--raw"]));
		failure(invoke("cat", &image, &["/one", "--raw", "--json"]));
		for path in ["one", "//one", "/one/", "/nested/../one", "/nested/./three"] {
			failure(invoke("stat", &image, &[path, "--raw", "--json"]));
		}
		for args in [
			["--offset", "18446744073709551615"],
			["--limit", "0"],
			["--limit", "65535"],
		] {
			failure(invoke("ls", &image, &["--raw", "--json", args[0], args[1]]));
		}
		failure(invoke(
			"tree",
			&image,
			&["--raw", "--json", "--depth", "65535"],
		));
		assert_eq!(fs::read(&image).unwrap(), bytes);
		assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
	}

	#[test]
	fn raw_mbr_with_two_filesystems_requires_an_explicit_partition() {
		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("two-volumes.raw");
		let catalog = hfsplus_fixture(FIXTURE_CATALOG);
		let hard_links =
			materialize_sparse(&hfsplus_fixture(FIXTURE_HARD_LINKS), HARD_LINK_VOLUME_BYTES);
		let bytes = two_partition_mbr(&catalog, &hard_links);
		fs::write(&image, &bytes).unwrap();

		let diagnostic = failure(invoke("ls", &image, &["--raw", "--json"]));
		assert!(diagnostic.contains("explicit selector"), "{diagnostic}");
		assert_eq!(
			success(invoke(
				"cat",
				&image,
				&["/nEsTeD/rÉSUMÉ.TXT", "--raw", "--partition", "1"],
			)),
			UNICODE_CATALOG_DATA,
		);
		assert_eq!(
			success(invoke(
				"cat",
				&image,
				&["/one", "--raw", "--partition", "2"]
			)),
			HARD_LINK_DATA,
		);
		assert_eq!(
			report("stat", &image, "/one", &["--raw", "--partition", "2"])["volume"],
			serde_json::json!({"kind": "partition", "id": {"scheme": "mbr", "slot": 2}}),
		);
		failure(invoke(
			"cat",
			&image,
			&["/one", "--raw", "--partition", "1"],
		));
		failure(invoke(
			"cat",
			&image,
			&["/one", "--raw", "--partition", "3"],
		));
		assert_eq!(fs::read(&image).unwrap(), bytes);
	}

	fn two_partition_mbr(first: &[u8], second: &[u8]) -> Vec<u8> {
		let mut boot_sector = [0; SECTOR_BYTES];
		let signature = SECTOR_BYTES - MBR_SIGNATURE.len();
		boot_sector[signature..].copy_from_slice(&MBR_SIGNATURE);
		let mut next_sector = 1_u32;
		for (index, volume) in [first, second].into_iter().enumerate() {
			assert_eq!(volume.len() % SECTOR_BYTES, 0);
			let sectors = u32::try_from(volume.len() / SECTOR_BYTES).unwrap();
			let start = MBR_TABLE + index * MBR_ENTRY_BYTES;
			let entry = &mut boot_sector[start..][..MBR_ENTRY_BYTES];
			entry[MBR_TYPE_FIELD] = MBR_HFS_TYPE;
			entry[MBR_FIRST_SECTOR_FIELD..][..size_of::<u32>()]
				.copy_from_slice(&next_sector.to_le_bytes());
			entry[MBR_SECTORS_FIELD..][..size_of::<u32>()].copy_from_slice(&sectors.to_le_bytes());
			next_sector += sectors;
		}
		[boot_sector.as_slice(), first, second].concat()
	}

	#[test]
	fn native_hfsx_preserves_distinct_case_variants() {
		const CAPITAL_DATA: &[u8] = b"capital\0\x01\xff\n";
		const LOWERCASE_DATA: &[u8] = b"lowercase\0\x02\xfe\n";

		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("case-sensitive.raw");
		let bytes = materialize_sparse(
			&hfsplus_fixture(FIXTURE_CASE_SENSITIVE),
			CASE_SENSITIVE_VOLUME_BYTES,
		);
		fs::write(&image, &bytes).unwrap();

		assert_eq!(
			names(&report("ls", &image, "/", &["--raw"])),
			["Cases", "cases"]
		);
		assert_eq!(
			success(invoke("cat", &image, &["/Cases/Case.txt", "--raw"])),
			CAPITAL_DATA,
		);
		assert_eq!(
			success(invoke("cat", &image, &["/Cases/case.txt", "--raw"])),
			LOWERCASE_DATA,
		);
		failure(invoke("cat", &image, &["/Cases/CASE.txt", "--raw"]));
		failure(invoke("cat", &image, &["/cases/Case.txt", "--raw"]));
		assert_eq!(fs::read(&image).unwrap(), bytes);
	}

	#[test]
	fn case_folded_hfsx_preserves_the_native_spelling_for_an_alias_lookup() {
		let document: Value = serde_json::from_slice(&hfsplus_fixture(CASE_FOLDED_LEDGER)).unwrap();
		assert_eq!(document["base_fixture"], FIXTURE_CATALOG);
		let mut bytes = hfsplus_fixture(FIXTURE_CATALOG);
		let overlays = document["derivation"]["byte_overlays"].as_array().unwrap();
		assert!(!overlays.is_empty());
		for entry in overlays {
			let offset = usize::try_from(entry["offset"].as_u64().unwrap()).unwrap();
			let value = u8::try_from(entry["value"].as_u64().unwrap()).unwrap();
			assert_ne!(bytes[offset], value);
			bytes[offset] = value;
		}

		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("case-folded.raw");
		fs::write(&image, &bytes).unwrap();
		let path = "/nEsTeD/rÉSUMÉ.TXT";
		let file = report("stat", &image, path, &["--raw"]);
		assert_eq!(stored_name(&file["name"]), "Re\u{301}sume\u{301}.txt");
		assert_eq!(
			success(invoke("cat", &image, &[path, "--raw"])),
			UNICODE_CATALOG_DATA
		);
		assert_eq!(fs::read(&image).unwrap(), bytes);
	}

	#[cfg(feature = "project-build")]
	#[test]
	fn built_dmg_escapes_terminal_names_and_preserves_inert_links() {
		const CONTROL_NAME: &str = "terminal\u{1b}[31m.bin";
		const ESCAPE: u8 = 0x1b;
		const PAYLOAD: &[u8] = b"\0\xff\r\n";
		const LINK_TARGET: &[u8] = b"/Data";
		const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Query Witness"
output = "query.dmg"
format = "udro"
size = "1MiB"
[metadata]
preserve_timestamps = false
preserve_extended_attributes = false
[[contents]]
source = "payload.bin"
destination = "/Data/terminal\u001b[31m.bin"
[[contents]]
type = "symlink"
destination = "/Link"
target = "/Data"
"#;
		let directory = tempfile::tempdir().unwrap();
		let image = directory.path().join("query.dmg");
		let project = directory.path().join("query.toml");
		let source = directory.path().join("payload.bin");
		fs::write(&project, SETTINGS).unwrap();
		fs::write(&source, PAYLOAD).unwrap();
		success(
			Command::new(BINARY)
				.arg("build")
				.arg(&project)
				.output()
				.unwrap(),
		);
		let bytes = fs::read(&image).unwrap();

		let text = success(invoke("ls", &image, &["/Data"]));
		assert!(!text.contains(&ESCAPE));
		assert!(String::from_utf8_lossy(&text).contains("terminal"));
		assert_eq!(names(&report("ls", &image, "/Data", &[])), [CONTROL_NAME]);
		let link = report("stat", &image, "/Link", &[]);
		assert_eq!(link["kind"], "symlink");
		assert_eq!(
			link["link_target"],
			serde_json::to_value(LINK_TARGET).unwrap()
		);
		let part = format!("/Link/{CONTROL_NAME}");
		let diagnostic = failure(invoke("stat", &image, &[&part, "--json"]));
		assert!(!diagnostic.as_bytes().contains(&ESCAPE));
		failure(invoke("cat", &image, &[&part]));
		failure(invoke("cat", &image, &["/Link"]));
		failure(invoke("ls", &image, &["/Link", "--json"]));
		let actual = format!("/Data/{CONTROL_NAME}");
		assert_eq!(success(invoke("cat", &image, &[&actual])), PAYLOAD);
		let tree = report("tree", &image, "/", &["--depth", "2"]);
		assert_eq!(
			names(&tree)
				.iter()
				.filter(|name| *name == CONTROL_NAME)
				.count(),
			1,
		);
		assert_eq!(fs::read(&image).unwrap(), bytes);
		assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
		assert_eq!(fs::read_to_string(&project).unwrap(), SETTINGS);
		assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
	}
}
