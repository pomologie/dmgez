use std::path::PathBuf;

use super::{
	native_mutations::{edit_object, replace_field},
	*,
};

// The fixture ledger records fsroot block 206. The native shared inode's
// value starts at byte 3291 in that root. Verify the old fields before edits.
const FSROOT_BLOCK: u64 = 206;
const HARD_LINK_INODE_VALUE_OFFSET: usize = 3291;
const INODE_LINK_COUNT_OFFSET: usize = HARD_LINK_INODE_VALUE_OFFSET + 56;
const INODE_BSD_FLAGS_OFFSET: usize = HARD_LINK_INODE_VALUE_OFFSET + 68;
const NATIVE_LINK_COUNT: u32 = 3;
const INCONSISTENT_LINK_COUNT: u32 = 2;
const NATIVE_BSD_FLAGS: u32 = 0;
const BSD_COMPRESSED: u32 = 1 << 5;
const KOLY_TRAILER_BYTES: usize = 512;
const KOLY_SIGNATURE: &[u8; 4] = b"koly";
const KOLY_DATA_CHECKSUM_FIELD: usize = 80;
const CHECKSUM_VALUE_OFFSET: usize = 2 * size_of::<u32>();

#[test]
fn wrapped_native_apfs_reports_reachable_data_after_udif_checksums_pass() {
	let directory = tempfile::tempdir().unwrap();
	let image = wrap(directory.path(), "native");
	let original = fs::read(&image).unwrap();
	let report = verify(
		&image,
		&[
			"--volume",
			HARD_LINK_VOLUME_OID,
			"--xid",
			HARD_LINK_CHECKPOINT_XID,
		],
		false,
	);

	assert_checked_udif(&report);
	assert_native_reachable_data(&report);
	assert_eq!(fs::read(&image).unwrap(), original);
}

#[test]
fn distinct_udif_crc_and_apfs_consistency_failures_stay_in_their_own_stages() {
	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("bad-link-count.raw");
	materialize(&raw, HARD_LINK_FIXTURE, &[]);
	mutate_fsroot(
		&raw,
		INODE_LINK_COUNT_OFFSET,
		NATIVE_LINK_COUNT,
		INCONSISTENT_LINK_COUNT,
	);
	let image = wrap_raw(&raw);
	let report = verify(&image, &[], false);
	assert_checked_udif(&report);
	assert_filesystem_failure(&report, "corrupt", "failed");
	assert_eq!(report["outcome"], "failed");
	assert!(report["filesystem_check"].is_object());

	// Alter the CRC declaration on the already-damaged filesystem. The
	// container check must take precedence before APFS opens a volume.
	let mut bytes = fs::read(&image).unwrap();
	let trailer = bytes.len() - KOLY_TRAILER_BYTES;
	assert_eq!(&bytes[trailer..][..KOLY_SIGNATURE.len()], KOLY_SIGNATURE);
	bytes[trailer + KOLY_DATA_CHECKSUM_FIELD + CHECKSUM_VALUE_OFFSET] ^= 1;
	fs::write(&image, &bytes).unwrap();
	let report = verify(&image, &[], false);
	assert_eq!(report["outcome"], "failed");
	assert_eq!(report["coverage"]["structure"], "passed");
	assert_eq!(report["coverage"]["payload"], "failed");
	assert_eq!(report["coverage"]["filesystem"], "not_checked");
	assert_eq!(report["diagnostic"]["stage"], "payload");
	assert_eq!(report["diagnostic"]["category"], "verification_failed");
	assert_eq!(report["diagnostic"]["detail"]["kind"], "checksum_mismatch");
	assert!(report.get("filesystem_check").is_none());
	assert_eq!(fs::read(&image).unwrap(), bytes);
}

#[test]
fn wrapped_unsupported_apfs_payload_remains_distinct_from_corruption() {
	let directory = tempfile::tempdir().unwrap();
	let raw = directory.path().join("compressed-inode.raw");
	materialize(&raw, HARD_LINK_FIXTURE, &[]);
	mutate_fsroot(
		&raw,
		INODE_BSD_FLAGS_OFFSET,
		NATIVE_BSD_FLAGS,
		BSD_COMPRESSED,
	);
	let image = wrap_raw(&raw);
	let report = verify(&image, &[], false);

	assert_checked_udif(&report);
	assert_filesystem_failure(&report, "unsupported_variant", "unsupported");
	assert_eq!(report["outcome"], "partial");
	assert!(report["filesystem_check"].is_object());
}

fn assert_checked_udif(report: &Value) {
	assert_eq!(report["coverage"]["structure"], "passed", "{report:#}");
	assert_eq!(report["coverage"]["payload"], "passed", "{report:#}");
	assert_eq!(report["capabilities"]["container_checksums"], true);
	assert_eq!(report["checksums"]["data_fork"], "passed");
	assert_eq!(report["checksums"]["master"], "passed");
	assert!(
		report["checksums"]["tables"]
			.as_array()
			.unwrap()
			.iter()
			.all(|entry| entry == "passed")
	);
}

fn wrap(root: &Path, name: &str) -> PathBuf {
	let raw = root.join(name).with_extension("raw");
	materialize(&raw, HARD_LINK_FIXTURE, &[]);
	wrap_raw(&raw)
}

fn wrap_raw(raw: &Path) -> PathBuf {
	let image = raw.with_extension("dmg");
	let output = Command::new(BINARY)
		.arg("wrap-raw")
		.arg(raw)
		.args(["--format", "udzo", "-o"])
		.arg(&image)
		.output()
		.unwrap();
	assert_status(&output, true);
	image
}

fn mutate_fsroot(raw: &Path, offset: usize, original: u32, replacement: u32) {
	edit_object(raw, FSROOT_BLOCK, |block| {
		replace_field(
			block,
			offset,
			original.to_le_bytes(),
			replacement.to_le_bytes(),
		);
	});
}
