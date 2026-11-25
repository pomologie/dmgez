use std::{fs, path::Path};

use dmgez_hfsplus::browse::{Limits, with_view};
use dmgez_udif::{
	TRAILER_BYTES,
	reader::Image,
	validate_unsigned_signature_fields,
	writer::{WriteOptions, write_image},
};
use dmgez_vfs::{
	Disk, Filesystem, Layout, VolumeId,
	volume::{ForkKind, LookupName},
};

use super::{BINARY, CHANGES, Command, EDITED, ORIGINAL, assert_failure, edit, rebuild::json};

const PROJECT: &str = "dmg.toml";
const UPPER_PAYLOAD: &[u8] = b"upper case payload";
const LOWER_PAYLOAD: &[u8] = b"lower case payload";
#[cfg(unix)]
const HARD_LINK_SOURCE: &str = "alias.txt";
#[cfg(unix)]
const HARD_LINK_NAME: &str = "Alias.txt";
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "HFSX Edit Fixture"
output = "original.dmg"
format = "udzo"
filesystem = "hfsx"
[metadata]
policy = "report"
preserve_timestamps = false
[[contents]]
source = "upper.txt"
destination = "/Files/Case.txt"
[[contents]]
source = "lower.txt"
destination = "/Files/case.txt"
"#;
const WRONG_CASE: &str = r#"
schema_version = 1
[[edits]]
operation = "remove"
target = "/Files/CASE.txt"
"#;
const BATCH: &str = r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "/Files/Case.txt"
destination = "/files/Case.txt"
[[edits]]
operation = "add_directory"
destination = "/files"
"#;

#[test]
fn rebuilds_hfsx_with_exact_case_edits_and_preserves_distinct_forks() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	build(root);
	#[cfg(target_os = "macos")]
	{
		let signed = Command::new("/usr/bin/codesign")
			.current_dir(root)
			.args(["--sign", "-", ORIGINAL])
			.output()
			.unwrap();
		assert!(signed.status.success(), "{signed:?}");
	}
	let source = fs::read(root.join(ORIGINAL)).unwrap();
	fs::write(root.join(CHANGES), WRONG_CASE).unwrap();
	assert_failure(edit(root, EDITED, &[]), "absent from the original image");
	assert!(!root.join(EDITED).exists());
	fs::write(root.join(CHANGES), BATCH).unwrap();
	let dry_run = json(edit(root, EDITED, &["--dry-run", "--json"]));
	assert_eq!(dry_run["published"], false);
	#[cfg(target_os = "macos")]
	assert_eq!(dry_run["source_envelope"], "signed_digests_verified");
	assert!(!root.join(EDITED).exists());
	let published = json(edit(root, EDITED, &["--json"]));
	assert_eq!(published["published"], true);
	assert_eq!(published["verified"], true);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), source);
	let rebuilt = fs::read(root.join(EDITED)).unwrap();
	assert!(
		validate_unsigned_signature_fields(rebuilt.last_chunk::<TRAILER_BYTES>().unwrap()).is_ok()
	);
	check_catalog(&rebuilt);
	#[cfg(target_os = "macos")]
	{
		let verified = Command::new("/usr/bin/hdiutil")
			.current_dir(root)
			.args(["verify", "-nocache", "-quiet", EDITED])
			.output()
			.unwrap();
		assert!(verified.status.success(), "{verified:?}");
	}
}

#[test]
fn refuses_case_folded_hfsx_before_publication() {
	const HEADER_OFFSET: usize = 1024;
	const HFS_PLUS: &[u8; 4] = b"H+\x00\x04";
	const HFSX: &[u8; 4] = b"HX\x00\x05";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	super::rebuild::build(root, false);
	let source = fs::read(root.join(ORIGINAL)).unwrap();
	let mut raw = Vec::new();
	Image::open(source.as_slice(), Default::default())
		.unwrap()
		.write_raw_to(&mut raw)
		.unwrap();
	// Retain the complete case-folded catalog and change both header identities.
	for offset in [HEADER_OFFSET, raw.len() - HEADER_OFFSET] {
		let identity = &mut raw[offset..][..HFSX.len()];
		assert_eq!(identity, HFS_PLUS);
		identity.copy_from_slice(HFSX);
	}
	let mut fixture = Vec::new();
	write_image(
		raw.as_slice(),
		raw.len() as u64,
		&mut fixture,
		&WriteOptions::default(),
	)
	.unwrap();
	fs::write(root.join(ORIGINAL), &fixture).unwrap();
	assert_failure(
		edit(root, EDITED, &[]),
		"volume with supported header fields",
	);
	assert!(!root.join(EDITED).exists());
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), fixture);
}

fn build(root: &Path) {
	fs::write(root.join("upper.txt"), UPPER_PAYLOAD).unwrap();
	fs::write(root.join("lower.txt"), LOWER_PAYLOAD).unwrap();
	#[cfg(unix)]
	let settings = {
		fs::hard_link(root.join("upper.txt"), root.join(HARD_LINK_SOURCE)).unwrap();
		format!(
			"{SETTINGS}\n[[contents]]\nsource = '{HARD_LINK_SOURCE}'\ndestination = '/{HARD_LINK_NAME}'\n"
		)
	};
	#[cfg(not(unix))]
	let settings = SETTINGS;
	fs::write(root.join(PROJECT), settings).unwrap();
	let build = Command::new(BINARY)
		.current_dir(root)
		.args(["build", PROJECT])
		.output()
		.unwrap();
	assert!(build.status.success(), "{build:?}");
}

fn check_catalog(bytes: &[u8]) {
	let image = Image::open(bytes, Default::default()).unwrap();
	let mut disk = Disk::discover(image, Default::default()).unwrap();
	assert!(matches!(
		disk.layout(),
		Layout::Partitionless {
			filesystem: Filesystem::HfsX,
			..
		}
	));
	with_view(&mut disk, VolumeId::WholeDisk, Limits::default(), |view| {
		let root = view.root().unwrap();
		for (parent, name, absent, expected) in [
			("Files", "case.txt", "Case.txt", LOWER_PAYLOAD),
			("files", "Case.txt", "case.txt", UPPER_PAYLOAD),
		] {
			let directory = view
				.lookup(&root, LookupName::Utf8(parent))
				.unwrap()
				.unwrap()
				.node;
			assert!(
				view.lookup(&directory, LookupName::Utf8(absent))
					.unwrap()
					.is_none()
			);
			let file = view
				.lookup(&directory, LookupName::Utf8(name))
				.unwrap()
				.unwrap()
				.node;
			#[cfg(unix)]
			if expected == UPPER_PAYLOAD {
				let alias = view
					.lookup(&root, LookupName::Utf8(HARD_LINK_NAME))
					.unwrap()
					.unwrap()
					.node;
				let group = view.stat(&file).unwrap().hard_link.unwrap();
				assert_eq!(group.count.get(), 2);
				assert_eq!(view.stat(&alias).unwrap().hard_link, Some(group));
			}
			view.with_fork(&file, ForkKind::Data, |reader| {
				assert_eq!(reader.len(), expected.len() as u64);
				let mut actual = vec![0; expected.len()];
				reader.read_exact_at(0, &mut actual).unwrap();
				assert_eq!(actual, expected);
			})
			.unwrap()
			.unwrap();
		}
	})
	.unwrap();
}
