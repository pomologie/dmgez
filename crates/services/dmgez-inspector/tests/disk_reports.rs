//! Structured disk portions of D-READ-01-T01 and D-READ-03-T01.

use std::{cell::Cell, io};

use dmgez_inspector::{
	CheckStatus, DiagnosticDetail, DiskLimits, DiskReport, DiskScheme, ErrorCategory,
	InspectionReport, RunWindow, Stage, inspect, inspect_disk,
};
use dmgez_udif::{
	Error, SECTOR_BYTES,
	reader::{Image, Limits, ReadAt},
};

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const LOGICAL_BYTES: u64 = 2048;
const MBR_BYTES: usize = SECTOR_BYTES as usize;
const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];
const FIRST_ENTRY: usize = 446;
const ENTRY_TYPE: usize = 4;
const ENTRY_START: usize = 8;
const ENTRY_SECTORS: usize = 12;
const ENTRY_BYTES: usize = 16;
const GPT_PROTECTIVE: u8 = 0xEE;
const HFS_PARTITION: u8 = 0xAF;

fn inspect_fixture(fixture: &[u8], limits: DiskLimits) -> InspectionReport {
	let mut image = Image::open(fixture, Limits::default()).unwrap();
	inspect_disk(&mut image, RunWindow::new(0, 1), limits)
}

#[test]
fn requested_unknown_layout_is_distinct_from_container_only_inspection() {
	let mut image = Image::open(UDRO, Limits::default()).unwrap();
	let page = RunWindow::new(0, 1);
	let container_only = inspect(&image, page);
	assert!(container_only.disk.is_none());
	assert!(
		serde_json::to_value(&container_only)
			.unwrap()
			.get("disk")
			.is_none()
	);
	let disk = inspect_disk(&mut image, page, DiskLimits::default());
	assert_eq!(disk.disk, Some(DiskReport::Unknown));
	assert_eq!(disk.container, container_only.container);
	assert_eq!(disk.run_page, container_only.run_page);
	assert_eq!(disk.coverage, container_only.coverage);
}

#[test]
fn corrupt_partition_metadata_and_probe_limits_preserve_container_observations() {
	let mut corrupt = UDRO.to_vec();
	let offset = Image::open(UDRO, Limits::default())
		.unwrap()
		.descriptor()
		.data_fork
		.start as usize;
	let boot_record: &mut [u8; MBR_BYTES] = (&mut corrupt[offset..offset + MBR_BYTES])
		.try_into()
		.unwrap();
	boot_record.fill(0);
	put_mbr_entry(boot_record, 0, GPT_PROTECTIVE);

	let report = inspect_fixture(&corrupt, DiskLimits::default());
	assert_eq!(report.container.logical_bytes, LOGICAL_BYTES);
	assert_eq!(report.container.block_map_count, 1);
	assert_eq!(report.run_page.as_ref().unwrap().runs.len(), 1);
	assert_eq!(report.coverage.structure, CheckStatus::NotChecked);
	assert_eq!(report.coverage.payload, CheckStatus::NotChecked);
	assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
	let diagnostic = unresolved(&report);
	assert_eq!(diagnostic.stage, Stage::Structure);
	assert_eq!(diagnostic.category, ErrorCategory::Corrupt);
	assert_eq!(
		diagnostic.detail,
		Some(DiagnosticDetail::PartitionMap {
			scheme: DiskScheme::Gpt
		})
	);

	let limited = inspect_fixture(
		UDRO,
		DiskLimits {
			max_probe_bytes: 0,
			..DiskLimits::default()
		},
	);
	assert_eq!(limited.container, report.container);
	assert_eq!(unresolved(&limited).category, ErrorCategory::ResourceLimit);
}

#[test]
fn overlapping_map_has_conflict_category_and_exposes_no_entries() {
	let mut image_bytes = UDRO.to_vec();
	let offset = Image::open(UDRO, Limits::default())
		.unwrap()
		.descriptor()
		.data_fork
		.start as usize;
	let boot_record: &mut [u8; MBR_BYTES] = (&mut image_bytes[offset..offset + MBR_BYTES])
		.try_into()
		.unwrap();
	boot_record.fill(0);
	put_mbr_entry(boot_record, 0, HFS_PARTITION);
	put_mbr_entry(boot_record, 1, HFS_PARTITION);
	let report = inspect_fixture(&image_bytes, DiskLimits::default());
	assert_eq!(unresolved(&report).category, ErrorCategory::Conflict);
	let json = serde_json::to_value(&report).unwrap();
	assert_eq!(json["disk"]["diagnostic"]["category"], "conflict");
	assert!(json["disk"].get("partitions").is_none());
}

#[test]
fn source_error_is_reported_without_serializing_host_details() {
	const PRIVATE_REASON: &str = "private host resource";
	struct RestrictedPayload;

	impl ReadAt for RestrictedPayload {
		fn len(&self) -> u64 {
			UDRO.len() as u64
		}

		fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
			if offset < LOGICAL_BYTES && !buffer.is_empty() {
				return Err(io::Error::new(io::ErrorKind::PermissionDenied, PRIVATE_REASON).into());
			}
			UDRO.read_exact_at(offset, buffer)
		}
	}

	let mut image = Image::open(RestrictedPayload, Limits::default()).unwrap();
	let report = inspect_disk(&mut image, None, DiskLimits::default());
	assert_eq!(report.container.logical_bytes, LOGICAL_BYTES);
	assert_eq!(unresolved(&report).stage, Stage::Payload);
	assert_eq!(
		unresolved(&report).category,
		ErrorCategory::PermissionDenied
	);
	assert!(
		!serde_json::to_string(&report)
			.unwrap()
			.contains(PRIVATE_REASON)
	);
}

#[test]
fn source_change_retains_admitted_metadata_without_a_successful_check() {
	struct ChangingPayload<'a>(&'a Cell<bool>);

	impl ReadAt for ChangingPayload<'_> {
		fn len(&self) -> u64 {
			UDRO.len() as u64
		}

		fn check_unchanged(&self) -> Result<(), Error> {
			if self.0.get() {
				Err(Error::SourceChanged)
			} else {
				Ok(())
			}
		}

		fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), Error> {
			self.check_unchanged()?;
			UDRO.read_exact_at(offset, buffer)
		}
	}

	let changed = Cell::new(false);
	let mut image = Image::open(ChangingPayload(&changed), Limits::default()).unwrap();
	let baseline = inspect(&image, None);
	changed.set(true);
	let report = inspect_disk(&mut image, None, DiskLimits::default());
	assert_eq!(unresolved(&report).category, ErrorCategory::SourceChanged);
	assert_eq!(report.coverage, baseline.coverage);
	assert_eq!(report.container, baseline.container);
}

fn put_mbr_entry(boot_record: &mut [u8; MBR_BYTES], slot: usize, code: u8) {
	boot_record[MBR_BYTES - MBR_SIGNATURE.len()..].copy_from_slice(&MBR_SIGNATURE);
	let entry = &mut boot_record[FIRST_ENTRY + slot * ENTRY_BYTES..][..ENTRY_BYTES];
	entry[ENTRY_TYPE] = code;
	entry[ENTRY_START..][..size_of::<u32>()].copy_from_slice(&1u32.to_le_bytes());
	entry[ENTRY_SECTORS..][..size_of::<u32>()].copy_from_slice(&1u32.to_le_bytes());
}

fn unresolved(report: &InspectionReport) -> &dmgez_inspector::Diagnostic {
	let Some(DiskReport::Unresolved { diagnostic }) = &report.disk else {
		panic!("expected disk failure, got {:?}", report.disk);
	};
	diagnostic
}

#[cfg(feature = "zlib")]
mod native {
	use super::*;

	const GPT: &[u8] =
		include_bytes!("../../../foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg");
	const APM: &[u8] =
		include_bytes!("../../../foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	const HFS: &[u8] =
		include_bytes!("../../../foundation/dmgez-vfs/tests/fixtures/native-raw.dmg");
	const OPTICAL: &[u8] =
		include_bytes!("../../../foundation/dmgez-vfs/tests/fixtures/native-dual-apm.dmg");

	#[test]
	fn native_partition_reports_keep_slots_logical_ranges_and_guid_byte_order() {
		let report = inspect_fixture(GPT, DiskLimits::default());
		let disk = serde_json::to_value(&report).unwrap()["disk"].clone();
		let entries = disk["partitions"].as_array().unwrap();
		assert_eq!(disk["kind"], "partitioned");
		assert_eq!(disk["scheme"], "gpt");
		assert_eq!(disk["logical_block_bytes"], SECTOR_BYTES);
		assert_ne!(report.container.block_map_count, entries.len());
		assert_eq!(entries.len(), 1);
		let entry = &entries[0];
		assert_eq!(entry["id"], serde_json::json!({"scheme":"gpt","slot":1}));
		assert_eq!(
			entry["logical"],
			serde_json::json!({"offset_bytes":"20480","length_bytes":"16736256"})
		);
		assert_eq!(entry["logical_data"], entry["logical"]);
		assert!(entry.get("physical").is_none());
		assert_eq!(
			entry["details"]["type_guid"],
			"48465300-0000-11aa-aa11-00306543ecac"
		);
		assert_eq!(
			entry["details"]["unique_guid"],
			"2c3748f1-e663-4c61-992e-05f10b43b11c"
		);
		assert!(entry["details"]["attributes"].is_string());

		let report = inspect_fixture(APM, DiskLimits::default());
		let disk = serde_json::to_value(&report).unwrap()["disk"].clone();
		let entries = disk["partitions"].as_array().unwrap();
		assert_eq!(disk["scheme"], "apm");
		assert_eq!(entries[0]["role"], "metadata");
		assert!(entries[0].get("logical_data").is_none());
		assert_eq!(entries[1]["id"]["slot"], 2);
		assert_eq!(entries[1]["details"]["type_code"], "Apple_HFS");
		assert_eq!(entries[1]["logical_data"]["offset_bytes"], "32768");
	}

	#[test]
	fn partitionless_hfs_and_unsupported_optical_map_keep_filesystem_unchecked() {
		let report = inspect_fixture(HFS, DiskLimits::default());
		let disk = serde_json::to_value(&report).unwrap()["disk"].clone();
		assert_eq!(disk["kind"], "partitionless");
		assert_eq!(disk["filesystem"], "hfs_plus");
		assert_eq!(disk["logical"]["length_bytes"], "16777216");
		assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);

		let report = inspect_fixture(OPTICAL, DiskLimits::default());
		assert_eq!(
			unresolved(&report).category,
			ErrorCategory::UnsupportedVariant
		);
		assert_eq!(
			unresolved(&report).detail,
			Some(DiagnosticDetail::PartitionMap {
				scheme: DiskScheme::Apm
			})
		);
		assert!(report.container.block_map_count > 1);
		assert_eq!(report.coverage.payload, CheckStatus::NotChecked);
		assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
	}
}

#[cfg(not(feature = "zlib"))]
#[test]
fn disabled_decoder_leaves_container_summary_available() {
	const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
	let report = inspect_fixture(UDZO, DiskLimits::default());
	assert_eq!(report.container.logical_bytes, LOGICAL_BYTES);
	assert_eq!(report.container.runs.decode_unavailable, 1);
	assert_eq!(
		unresolved(&report).category,
		ErrorCategory::FeatureNotCompiled
	);
	assert_eq!(report.coverage.payload, CheckStatus::NotChecked);
}
