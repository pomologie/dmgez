use std::{fs, ops::Range, path::Path};

use dmgez_hfsplus::browse::{Limits, VolumeSignature, with_view_signature};
use dmgez_udif::{
	SECTOR_BYTES,
	reader::Image,
	writer::{Profile, WriteOptions, write_rebuilt_image},
};
use dmgez_vfs::{
	Disk, Layout, Scheme, VolumeId,
	volume::{ForkKind, LookupName},
};

use super::{BINARY, CHANGES, Command, EDITED, ORIGINAL, assert_failure, edit, rebuild::json};

const PROJECT: &str = "dmg.toml";
const IMAGE_BYTES: usize = 4 << 20;
const APM_VOLUME_START: usize = 32 << 10;
const GPT_EDGE_BYTES: usize = 20 << 10;
const MBR_VOLUME_START: usize = 4 << 10;
const PAYLOAD: &[u8] = b"moved payload\0\xff";
const RETAINED: &[u8] = b"retained payload\0\xfe";
const MOVE: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Files/Case.txt"
destination = "/Moved.txt"
"#;
const GROW: &str = r#"
schema_version = 1
[[edits]]
operation = "add_copy"
source = "oversized.bin"
destination = "/Big.bin"
"#;

#[test]
fn edits_the_single_volume_and_copies_all_non_volume_bytes_for_each_map() {
	for (scheme, partition, extent) in [
		(Scheme::Apple, "apm", APM_VOLUME_START..IMAGE_BYTES),
		(
			Scheme::Gpt,
			"gpt",
			GPT_EDGE_BYTES..IMAGE_BYTES - GPT_EDGE_BYTES,
		),
		(Scheme::Mbr, "mbr", MBR_VOLUME_START..IMAGE_BYTES),
	] {
		for (filesystem, signature, retained_name) in [
			("hfsplus", VolumeSignature::HfsPlus, "Other.txt"),
			("hfsx", VolumeSignature::HfsX, "case.txt"),
		] {
			for (format, profile) in [("udro", Profile::Udro), ("udzo", Profile::Udzo)] {
				let directory = tempfile::tempdir().unwrap();
				let root = directory.path();
				write_project(root, partition, filesystem, format, retained_name);
				if scheme == Scheme::Gpt {
					mark_gpt_padding(root, &extent, profile);
				}
				let source = fs::read(root.join(ORIGINAL)).unwrap();
				let original_raw = decode(&source, profile);
				assert_eq!(original_raw.len(), IMAGE_BYTES);
				let mut original_disk = Disk::discover(
					Image::open(source.as_slice(), Default::default()).unwrap(),
					Default::default(),
				)
				.unwrap();
				assert!(matches!(original_disk.layout(), Layout::Partitioned {
					scheme: actual, ..
				} if *actual == scheme));
				let selected = original_disk.select_volume(None).unwrap();
				assert!(matches!(selected, VolumeId::Partition(_)));
				assert_eq!(
					original_disk.data_window(selected).unwrap().logical_range(),
					extent.start as u64..extent.end as u64,
				);

				fs::write(root.join(CHANGES), MOVE).unwrap();
				let plan = json(edit(root, EDITED, &["--dry-run", "--json"]));
				assert_eq!(plan["published"], false);
				assert_eq!(plan["logical_bytes"], IMAGE_BYTES as u64);
				assert!(!root.join(EDITED).exists());
				let publication = json(edit(root, EDITED, &["--json"]));
				assert_eq!(publication["verified"], true);
				assert_eq!(publication["published"], true);
				assert_eq!(publication["logical_bytes"], plan["logical_bytes"]);
				assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), source);

				let rebuilt = fs::read(root.join(EDITED)).unwrap();
				let raw = decode(&rebuilt, profile);
				assert_eq!(raw.len(), original_raw.len());
				assert!(
					raw[..extent.start] == original_raw[..extent.start],
					"{partition} prefix changed"
				);
				assert!(
					raw[extent.end..] == original_raw[extent.end..],
					"{partition} suffix changed"
				);
				assert!(
					raw[extent.clone()] != original_raw[extent.clone()],
					"volume was not edited"
				);
				let mut rebuilt_disk = Disk::discover(
					Image::open(rebuilt.as_slice(), Default::default()).unwrap(),
					Default::default(),
				)
				.unwrap();
				assert_eq!(rebuilt_disk.layout(), original_disk.layout());
				check_catalog(&mut rebuilt_disk, selected, signature, retained_name);
				#[cfg(target_os = "macos")]
				apple_verify(root, &raw, (scheme == Scheme::Gpt).then_some(&extent));
			}
		}
	}
}

#[test]
fn refuses_a_rebuild_larger_than_its_existing_partition() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	write_project(root, "gpt", "hfsplus", "udzo", "Other.txt");
	let source = fs::read(root.join(ORIGINAL)).unwrap();
	fs::File::create(root.join("oversized.bin"))
		.unwrap()
		.set_len(2 * IMAGE_BYTES as u64)
		.unwrap();
	fs::write(root.join(CHANGES), GROW).unwrap();
	for options in [
		["--metadata-policy", "report"].as_slice(),
		["--metadata-policy", "report", "--dry-run"].as_slice(),
	] {
		assert_failure(
			edit(root, EDITED, options),
			"rebuilt filesystem does not fit the existing partition",
		);
		assert!(!root.join(EDITED).exists());
		assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), source);
	}
}

fn write_project(
	root: &Path,
	partition: &str,
	filesystem: &str,
	format: &str,
	retained_name: &str,
) {
	fs::write(root.join("payload.bin"), PAYLOAD).unwrap();
	fs::write(root.join("retained.bin"), RETAINED).unwrap();
	fs::write(
		root.join(PROJECT),
		format!(
			r#"
schema_version = 1
[image]
name = "Partition Edit Fixture"
output = "{ORIGINAL}"
partition = "{partition}"
filesystem = "{filesystem}"
format = "{format}"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
[[contents]]
source = "payload.bin"
destination = "/Files/Case.txt"
[[contents]]
source = "retained.bin"
destination = "/Files/{retained_name}"
"#,
		),
	)
	.unwrap();
	let output = Command::new(BINARY)
		.current_dir(root)
		.args(["build", PROJECT])
		.output()
		.unwrap();
	assert!(output.status.success(), "{output:?}");
}

fn decode(bytes: &[u8], profile: Profile) -> Vec<u8> {
	let mut image = Image::open(bytes, Default::default()).unwrap();
	assert_eq!(image.rebuild_metadata().unwrap().source_profile(), profile);
	let mut raw = Vec::new();
	image.write_raw_to(&mut raw).unwrap();
	raw
}

fn mark_gpt_padding(root: &Path, extent: &Range<usize>, profile: Profile) {
	let bytes = fs::read(root.join(ORIGINAL)).unwrap();
	let metadata = Image::open(bytes.as_slice(), Default::default())
		.unwrap()
		.rebuild_metadata()
		.unwrap();
	let mut raw = decode(&bytes, profile);
	// These sectors are between the filesystem and the GPT entry arrays.
	// A rebuilt or zero-filled partition envelope would discard the markers.
	for (range, marker) in gpt_padding_sectors(extent).into_iter().zip([0xa5, 0x5a]) {
		let sector = &mut raw[range];
		assert!(sector.iter().all(|byte| *byte == 0));
		sector.fill(marker);
	}
	let output = fs::File::create(root.join(ORIGINAL)).unwrap();
	let options = WriteOptions {
		profile,
		..WriteOptions::default()
	};
	write_rebuilt_image(raw.as_slice(), output, &options, metadata).unwrap();
}

fn gpt_padding_sectors(extent: &Range<usize>) -> [Range<usize>; 2] {
	let sector_bytes = SECTOR_BYTES as usize;
	[
		extent.start - sector_bytes..extent.start,
		extent.end..extent.end + sector_bytes,
	]
}

fn check_catalog(
	disk: &mut Disk<Image<&[u8]>>,
	selected: VolumeId,
	signature: VolumeSignature,
	retained_name: &str,
) {
	with_view_signature(disk, selected, Limits::default(), |view, observed| {
		assert_eq!(observed, signature);
		let root = view.root().unwrap();
		let folder = view
			.lookup(&root, LookupName::Utf8("Files"))
			.unwrap()
			.unwrap()
			.node;
		assert!(
			view.lookup(&folder, LookupName::Utf8("Case.txt"))
				.unwrap()
				.is_none()
		);
		for (parent, leaf, payload) in [
			(&root, "Moved.txt", PAYLOAD),
			(&folder, retained_name, RETAINED),
		] {
			let file = view
				.lookup(parent, LookupName::Utf8(leaf))
				.unwrap()
				.unwrap()
				.node;
			view.with_fork(&file, ForkKind::Data, |reader| {
				assert_eq!(reader.len(), payload.len() as u64);
				let mut actual = vec![0; payload.len()];
				reader.read_exact_at(0, &mut actual).unwrap();
				assert_eq!(actual, payload);
			})
			.unwrap()
			.unwrap();
		}
	})
	.unwrap();
}

#[cfg(target_os = "macos")]
fn apple_verify(root: &Path, expected_raw: &[u8], gpt_volume: Option<&Range<usize>>) {
	const HDIUTIL: &str = "/usr/bin/hdiutil";
	const RAW: &str = "apple.raw.dmg";
	for args in [
		["verify", "-nocache", "-quiet", EDITED].as_slice(),
		["convert", EDITED, "-format", "UDRW", "-o", RAW, "-quiet"].as_slice(),
	] {
		let output = Command::new(HDIUTIL)
			.current_dir(root)
			.args(args)
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
	let mut observed = fs::read(root.join(RAW)).unwrap();
	assert_eq!(observed.len(), expected_raw.len());
	if let Some(extent) = gpt_volume {
		// Apple's UDRW conversion can zero these unpartitioned padding sectors.
		// The direct decoded comparison checks their original contents above.
		for range in gpt_padding_sectors(extent) {
			let sector = &mut observed[range.clone()];
			if sector.iter().all(|byte| *byte == 0) {
				sector.copy_from_slice(&expected_raw[range]);
			}
		}
	}
	let first_difference = observed.iter().zip(expected_raw).position(|(a, b)| a != b);
	assert_eq!(first_difference, None, "Apple UDRW export differed");
}
