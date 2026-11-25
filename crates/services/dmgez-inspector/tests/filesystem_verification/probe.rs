use std::num::NonZeroU64;

use dmgez_inspector::{
	FilesystemCheckpointId, FilesystemProbe, FilesystemSource, FilesystemUuid,
	verify_filesystem_with_probe, verify_raw_filesystem,
};
use dmgez_vfs::Filesystem;

use super::*;

const EXTENT_BYTES: u64 = 2 * SECTOR_BYTES;
const RAW_BYTES: usize = 3 * SECTOR;
const PAYLOAD_MARKER: u8 = 0x37;
const NATIVE_UUID: FilesystemUuid = FilesystemUuid([
	0x51, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
]);
const CHECKPOINT: FilesystemCheckpointId = FilesystemCheckpointId(u64::MAX - 1);

fn raw_fixture() -> [u8; RAW_BYTES] {
	let mut raw = [0; RAW_BYTES];
	raw[0] = PAYLOAD_MARKER;
	raw
}

fn probe(source: &mut dyn FilesystemSource) -> Result<Option<NonZeroU64>, FilesystemCheckError> {
	let mut marker = [0];
	source.read_exact_at(0, &mut marker)?;
	assert_eq!(marker, [PAYLOAD_MARKER]);
	assert_eq!(source.len(), RAW_BYTES as u64);
	Ok(NonZeroU64::new(EXTENT_BYTES))
}

#[test]
fn reachable_data_names_the_selected_checkpoint_and_keeps_raw_coverage_partial() {
	let raw = raw_fixture();
	let evidence = FilesystemEvidence::checkpoint(
		FilesystemScope::ReachableData,
		FilesystemObjectId(u64::MAX),
		CHECKPOINT,
		NATIVE_UUID,
		Some(u64::MAX - 2),
	)
	.with_checkpoint_diagnostics(2, Some(FilesystemCheckpointId(u64::MAX)));
	let report = verify_raw_filesystem(
		raw.as_slice(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, probe),
		|window| {
			assert_eq!(window.len(), EXTENT_BYTES);
			Ok(evidence)
		},
	);
	assert_eq!(report.outcome, VerificationOutcome::Partial);
	assert_eq!(report.coverage.structure, CheckStatus::Passed);
	assert_eq!(report.coverage.payload, CheckStatus::NotPresent);
	assert_eq!(report.coverage.filesystem, CheckStatus::Partial);
	assert_eq!(report.coverage.security, CheckStatus::NotChecked);
	assert!(!report.capabilities.container_checksums);
	assert!(report.container.is_none());
	assert!(report.checksums.is_none());
	assert!(!report.filesystem_check.unwrap().all_filesystems_checked);
	let value = serde_json::to_value(&report).unwrap();
	let observed = &value["filesystem_check"]["evidence"];
	assert_eq!(observed["scope"], "reachable_data");
	assert_eq!(observed["volume_object_id"], u64::MAX.to_string());
	assert_eq!(observed["checkpoint_xid"], CHECKPOINT.0.to_string());
	assert_eq!(
		observed["volume_uuid"],
		"51020304-0506-0708-090a-0b0c0d0e0f10"
	);
	assert_eq!(
		observed["logical_payload_bytes_read"],
		(u64::MAX - 2).to_string()
	);
	let diagnostics = &observed["checkpoint_diagnostics"];
	assert_eq!(diagnostics["rejected_descriptors"], 2);
	assert_eq!(diagnostics["advertised_xid"], u64::MAX.to_string());
}

#[test]
fn recognized_partitionless_probe_and_checker_share_one_bounded_budget() {
	let image = wrap(&raw_fixture());
	let report = verify_filesystem_with_probe(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions {
			max_read_requests: 1,
			..FilesystemOptions::default()
		},
		FilesystemProbe::new(Filesystem::Apfs, None, probe),
		|source| {
			assert_eq!(source.len(), EXTENT_BYTES);
			assert_eq!(
				source.read_exact_at(0, &mut [0]),
				Err(FilesystemReadError::Budget)
			);
			Ok(COMPLETE)
		},
	);
	assert_eq!(report.coverage.payload, CheckStatus::Passed);
	assert_eq!(report.coverage.filesystem, CheckStatus::Unavailable);
	assert_eq!(
		report.diagnostic.unwrap().category,
		ErrorCategory::ResourceLimit
	);
	let observed = report.filesystem_check.unwrap();
	assert_eq!(observed.window.id, SelectedWindowId::WholeDisk);
	assert_eq!(observed.window.logical.length_bytes, EXTENT_BYTES);
	assert_eq!(observed.charged_read_requests, 1);
	assert_eq!(observed.charged_read_bytes, 1);
	assert!(observed.evidence.is_none());
}

#[test]
fn a_failed_probe_read_cannot_admit_a_filesystem() {
	let raw = raw_fixture();
	let report = verify_raw_filesystem(
		raw.as_slice(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, |window| {
			assert_eq!(
				window.read_exact_at(window.len(), &mut [0]),
				Err(FilesystemReadError::OutOfRange)
			);
			Ok(NonZeroU64::new(EXTENT_BYTES))
		}),
		|_| panic!("invalid probe reached checker"),
	);
	assert_eq!(report.coverage.structure, CheckStatus::Failed);
	assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
	assert!(report.filesystem_check.is_none());
	assert!(matches!(
		report.diagnostic.unwrap().detail,
		Some(DiagnosticDetail::FilesystemWindowRange { .. })
	));
}

#[test]
fn raw_source_identity_precedes_recognition_and_limits_successful_evidence() {
	let raw = raw_fixture();
	let changed = Cell::new(true);
	let source = || MutableSource {
		image: &raw,
		changed: &changed,
	};
	let initial = verify_raw_filesystem(
		source(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, |_| {
			panic!("changed source reached recognition")
		}),
		|_| panic!("changed source reached checker"),
	);
	assert!(matches!(initial.cause(), Some(Error::SourceChanged)));
	assert_eq!(initial.diagnostic.unwrap().stage, Stage::Structure);

	changed.set(false);
	let final_report = verify_raw_filesystem(
		source(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, probe),
		|_| {
			changed.set(true);
			Ok(COMPLETE)
		},
	);
	assert_eq!(final_report.outcome, VerificationOutcome::Failed);
	assert!(matches!(final_report.cause(), Some(Error::SourceChanged)));
	assert_eq!(final_report.diagnostic.unwrap().stage, Stage::Filesystem);
	assert!(final_report.filesystem_check.unwrap().evidence.is_none());
}

#[test]
fn genuine_whole_raw_filesystem_coverage_has_no_udif_domain() {
	let raw = raw_fixture();
	let report = verify_raw_filesystem(
		raw.as_slice(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, probe),
		|_| Ok(COMPLETE),
	);
	assert!(report.passed());
	assert_eq!(report.coverage.filesystem, CheckStatus::Passed);
	assert_eq!(report.coverage.payload, CheckStatus::NotPresent);
	assert!(report.filesystem_check.unwrap().all_filesystems_checked);
}

#[test]
fn explicit_partition_slot_reuses_the_admitted_map() {
	let image = fixture(true);
	let slot = NonZeroU32::new(2).unwrap();
	let report = verify_filesystem_with_probe(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, Some(slot), |_| {
			panic!("partitioned image reached partitionless recognizer")
		}),
		|window| {
			let mut byte = [0];
			window.read_exact_at(0, &mut byte)?;
			assert_eq!(byte, [SECOND_CONTENT]);
			Ok(COMPLETE)
		},
	);
	assert_eq!(report.outcome, VerificationOutcome::Partial);
	let observed = report.filesystem_check.unwrap();
	assert!(matches!(observed.window.id, SelectedWindowId::Partition { id } if id.slot == slot));
	assert_eq!(observed.charged_read_requests, 1);
	assert!(observed.evidence.is_some());
}

#[test]
fn native_volume_ambiguity_has_a_distinct_diagnostic() {
	let raw = raw_fixture();
	let report = verify_raw_filesystem(
		raw.as_slice(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, probe),
		|_| Err(FilesystemCheckError::SelectionRequired { candidates: 2 }),
	);
	assert_eq!(report.outcome, VerificationOutcome::Failed);
	assert_eq!(report.coverage.structure, CheckStatus::Passed);
	let diagnostic = report.diagnostic.unwrap();
	assert_eq!(diagnostic.stage, Stage::Filesystem);
	assert_eq!(diagnostic.category, ErrorCategory::Conflict);
	assert_eq!(
		diagnostic.detail,
		Some(DiagnosticDetail::FilesystemVolumeSelection { candidates: 2 })
	);
	assert!(report.filesystem_check.unwrap().evidence.is_none());
}

#[test]
fn corrupt_udif_payload_prevents_even_filesystem_recognition() {
	let mut image = wrap(&raw_fixture());
	image[0] ^= 1;
	let report = verify_filesystem_with_probe(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		FilesystemProbe::new(Filesystem::Apfs, None, |_| {
			panic!("bad payload reached recognizer")
		}),
		|_| panic!("bad payload reached checker"),
	);
	assert_eq!(report.coverage.payload, CheckStatus::Failed);
	assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
	assert!(report.filesystem_check.is_none());
}
