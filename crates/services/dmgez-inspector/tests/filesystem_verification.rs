//! Selected- and whole-image coverage cases for D-READ-04 and D-V09.

use std::{cell::Cell, num::NonZeroU32};

use dmgez_inspector::{
	CheckStatus, DiagnosticDetail, ErrorCategory, FilesystemCheckError, FilesystemEvidence,
	FilesystemObjectId, FilesystemOptions, FilesystemReadError, FilesystemScope, SelectedWindowId,
	Stage, VerificationOutcome, verify_filesystem,
};
use dmgez_udif::{
	Error, SECTOR_BYTES,
	reader::{Limits, ReadAt},
	writer::{Profile, WriteOptions, write_image},
};
use dmgez_vfs::{PartitionId, Scheme, VolumeId};

const SECTOR: usize = SECTOR_BYTES as usize;
const LOGICAL_SECTORS: usize = 10;
const PARTITION_SECTORS: u32 = 2;
const FIRST_PARTITION: u32 = 2;
const SECOND_PARTITION: u32 = 6;
const FIRST_CONTENT: u8 = 0xc1;
const SECOND_CONTENT: u8 = 0xc2;
const BOOT_SIGNATURE_OFFSET: usize = 510;
const MBR_SIGNATURE: [u8; 2] = [0x55, 0xaa];
const MBR_TABLE_OFFSET: usize = 446;
const MBR_ENTRY_BYTES: usize = 16;
const MBR_CODE: usize = 4;
const MBR_START: usize = 8;
const MBR_COUNT: usize = 12;
const HFS_PARTITION: u8 = 0xaf;
const COMPLETE: FilesystemEvidence = FilesystemEvidence::window(FilesystemScope::FullVolume);

fn fixture(include_second: bool) -> Vec<u8> {
	let mut raw = [0; LOGICAL_SECTORS * SECTOR];
	raw[BOOT_SIGNATURE_OFFSET..SECTOR].copy_from_slice(&MBR_SIGNATURE);
	for (slot, (start, payload)) in [
		(FIRST_PARTITION, FIRST_CONTENT),
		(SECOND_PARTITION, SECOND_CONTENT),
	]
	.into_iter()
	.take(1 + usize::from(include_second))
	.enumerate()
	{
		let row = &mut raw[MBR_TABLE_OFFSET + slot * MBR_ENTRY_BYTES..][..MBR_ENTRY_BYTES];
		row[MBR_CODE] = HFS_PARTITION;
		row[MBR_START..MBR_COUNT].copy_from_slice(&start.to_le_bytes());
		row[MBR_COUNT..].copy_from_slice(&PARTITION_SECTORS.to_le_bytes());
		let start = start as usize * SECTOR;
		raw[start..start + PARTITION_SECTORS as usize * SECTOR].fill(payload);
	}
	let mut image = Vec::new();
	let options = WriteOptions {
		profile: Profile::Udro,
		..WriteOptions::default()
	};
	write_image(raw.as_slice(), raw.len() as u64, &mut image, &options).unwrap();
	image
}

#[test]
fn complete_sole_window_is_rebased_and_keeps_security_unchecked() {
	let image = fixture(false);
	let report = verify_filesystem(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		|source| {
			assert_eq!(source.len(), u64::from(PARTITION_SECTORS) * SECTOR_BYTES);
			let mut bytes = [0; PARTITION_SECTORS as usize * SECTOR];
			source.read_exact_at(0, &mut bytes)?;
			assert_eq!(bytes, [FIRST_CONTENT; PARTITION_SECTORS as usize * SECTOR]);
			Ok(COMPLETE)
		},
	);
	assert!(report.passed());
	assert!(report.capabilities.filesystem);
	assert_eq!(report.coverage.filesystem, CheckStatus::Passed);
	assert_eq!(report.coverage.security, CheckStatus::NotChecked);
	let observed = report.filesystem_check.unwrap();
	assert_eq!(
		observed.window.logical.offset_bytes,
		u64::from(FIRST_PARTITION) * SECTOR_BYTES
	);
	assert_eq!(
		observed.window.logical.length_bytes,
		u64::from(PARTITION_SECTORS) * SECTOR_BYTES
	);
	assert_eq!(observed.charged_read_requests, 1);
	assert_eq!(
		observed.charged_read_bytes,
		observed.window.logical.length_bytes
	);
	assert!(observed.all_filesystems_checked);
}

#[test]
fn ambiguous_layout_requires_selection_and_selected_second_window_is_partial() {
	let image = fixture(true);
	let ambiguous = verify_filesystem(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		|_| panic!("ambiguous image reached checker"),
	);
	assert!(!ambiguous.passed());
	assert_eq!(ambiguous.coverage.payload, CheckStatus::Passed);
	assert_eq!(
		ambiguous.diagnostic.unwrap().detail,
		Some(DiagnosticDetail::VolumeSelection { candidates: 2 })
	);

	let slot = NonZeroU32::new(2).unwrap();
	let options = FilesystemOptions {
		volume: Some(VolumeId::Partition(PartitionId {
			scheme: Scheme::Mbr,
			slot,
		})),
		..FilesystemOptions::default()
	};
	let report = verify_filesystem(image.as_slice(), Limits::default(), options, |source| {
		let mut byte = [0];
		source.read_exact_at(0, &mut byte)?;
		assert_eq!(byte, [SECOND_CONTENT]);
		Ok(COMPLETE)
	});
	assert_eq!(report.outcome, VerificationOutcome::Partial);
	assert_eq!(report.coverage.filesystem, CheckStatus::Partial);
	let observed = report.filesystem_check.unwrap();
	assert!(matches!(observed.window.id, SelectedWindowId::Partition { id } if id.slot == slot));
	assert!(!observed.all_filesystems_checked);
	assert_eq!(observed.evidence, Some(COMPLETE));
}

#[test]
fn structural_and_native_subvolume_evidence_remain_partial() {
	let image = fixture(false);
	let native_id = FilesystemObjectId(u64::MAX);
	for evidence in [
		FilesystemEvidence::volume(FilesystemScope::ReachableStructure, native_id),
		FilesystemEvidence::window(FilesystemScope::VolumeStructure),
		FilesystemEvidence::volume(FilesystemScope::FullVolume, native_id),
	] {
		let report = verify_filesystem(
			image.as_slice(),
			Limits::default(),
			FilesystemOptions::default(),
			|_| Ok(evidence),
		);
		assert_eq!(report.coverage.filesystem, CheckStatus::Partial);
		assert_eq!(report.outcome, VerificationOutcome::Partial);
		let document = serde_json::to_value(&report).unwrap();
		if evidence.volume_object_id.is_some() {
			assert_eq!(
				document["filesystem_check"]["evidence"]["volume_object_id"],
				u64::MAX.to_string()
			);
		}
	}
}

#[test]
fn corrupt_container_never_reaches_filesystem_checker() {
	let mut image = fixture(false);
	image[0] ^= 1;
	let report = verify_filesystem(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		|_| panic!("bad payload reached checker"),
	);
	assert_eq!(report.coverage.payload, CheckStatus::Failed);
	assert_eq!(report.coverage.filesystem, CheckStatus::NotChecked);
	assert!(report.filesystem_check.is_none());
}

#[test]
fn checker_refusals_have_distinct_outcomes() {
	let image = fixture(false);
	for (error, expected) in [
		(FilesystemCheckError::Unsupported, CheckStatus::Unsupported),
		(FilesystemCheckError::Corrupt, CheckStatus::Failed),
		(FilesystemCheckError::Unavailable, CheckStatus::Unavailable),
	] {
		let report = verify_filesystem(
			image.as_slice(),
			Limits::default(),
			FilesystemOptions::default(),
			|_| Err(error),
		);
		assert!(!report.passed());
		assert_eq!(report.coverage.payload, CheckStatus::Passed);
		assert_eq!(report.coverage.filesystem, expected);
		assert_eq!(report.diagnostic.unwrap().stage, Stage::Filesystem);
		assert!(report.filesystem_check.unwrap().evidence.is_none());
	}
}

#[test]
fn budget_and_range_errors_remain_authoritative_when_checker_ignores_them() {
	let image = fixture(false);
	let options = FilesystemOptions {
		max_read_requests: 1,
		max_read_bytes: 1,
		..FilesystemOptions::default()
	};
	let limit = verify_filesystem(image.as_slice(), Limits::default(), options, |source| {
		source.read_exact_at(0, &mut [0])?;
		assert_eq!(
			source.read_exact_at(0, &mut []),
			Err(FilesystemReadError::Budget)
		);
		Ok(COMPLETE)
	});
	assert_eq!(limit.coverage.filesystem, CheckStatus::Unavailable);
	assert_eq!(
		limit.diagnostic.unwrap().category,
		ErrorCategory::ResourceLimit
	);
	assert_eq!(limit.filesystem_check.unwrap().charged_read_requests, 1);
	let out_of_range = verify_filesystem(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions::default(),
		|source| {
			assert_eq!(
				source.read_exact_at(source.len(), &mut [0]),
				Err(FilesystemReadError::OutOfRange)
			);
			Ok(COMPLETE)
		},
	);
	assert_eq!(out_of_range.coverage.filesystem, CheckStatus::Failed);
	assert!(matches!(
		out_of_range.diagnostic.unwrap().detail,
		Some(DiagnosticDetail::FilesystemWindowRange { .. })
	));

	let byte_limit = verify_filesystem(
		image.as_slice(),
		Limits::default(),
		FilesystemOptions {
			max_read_bytes: 1,
			..FilesystemOptions::default()
		},
		|source| {
			assert_eq!(
				source.read_exact_at(0, &mut [0; 2]),
				Err(FilesystemReadError::Budget)
			);
			Ok(COMPLETE)
		},
	);
	assert_eq!(byte_limit.coverage.filesystem, CheckStatus::Unavailable);
	assert_eq!(byte_limit.filesystem_check.unwrap().charged_read_bytes, 0);
}

#[test]
fn source_change_during_read_or_after_checker_success_prevents_a_pass() {
	struct MutableSource<'data> {
		image: &'data [u8],
		changed: &'data Cell<bool>,
	}
	impl ReadAt for MutableSource<'_> {
		fn len(&self) -> u64 {
			self.image.len() as u64
		}
		fn check_unchanged(&self) -> Result<(), Error> {
			if self.changed.get() {
				Err(Error::SourceChanged)
			} else {
				Ok(())
			}
		}
		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
			self.check_unchanged()?;
			self.image.read_exact_at(offset, output)
		}
	}
	let image = fixture(false);
	for read_after_change in [false, true] {
		let changed = Cell::new(false);
		let source = MutableSource {
			image: &image,
			changed: &changed,
		};
		let report = verify_filesystem(
			source,
			Limits::default(),
			FilesystemOptions::default(),
			|source| {
				source.read_exact_at(0, &mut [0])?;
				changed.set(true);
				if read_after_change {
					assert_eq!(
						source.read_exact_at(0, &mut [0]),
						Err(FilesystemReadError::Source)
					);
				}
				Ok(COMPLETE)
			},
		);
		assert_eq!(report.outcome, VerificationOutcome::Failed);
		assert!(matches!(report.cause(), Some(Error::SourceChanged)));
		assert_eq!(
			report.diagnostic.unwrap().category,
			ErrorCategory::SourceChanged
		);
		assert!(report.filesystem_check.unwrap().evidence.is_none());
	}
}
