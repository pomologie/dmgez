use std::{error::Error, num::NonZeroU32};

use dmgez_apfs::{
	CheckData, CheckFailureKind, CheckpointDiagnostic, TransactionId, VerificationEvidence,
	VolumeSelector, check, probe,
};
use dmgez_inspector::{
	FilesystemCheckError, FilesystemCheckpointId, FilesystemEvidence, FilesystemObjectId,
	FilesystemOptions, FilesystemProbe, FilesystemScope, FilesystemSource, FilesystemUuid,
};
use dmgez_udif::reader::ReadAt;
use dmgez_vfs::Filesystem;

use crate::disk::Encoding;

use super::{PayloadBudget, Verification};

/// Verifies reachable data in a selected APFS volume and records checkpoint evidence.
///
/// UDIF container checks run first for container inputs. A raw input has
/// no UDIF checksums, and the report records that distinction.
#[must_use]
pub fn verify_apfs(
	source: impl ReadAt,
	encoding: Encoding,
	partition: Option<NonZeroU32>,
	selector: Option<VolumeSelector>,
	xid: Option<TransactionId>,
	budget: PayloadBudget,
) -> Verification {
	let data = CheckData::Read {
		max_bytes: budget.bytes(),
	};
	let mut filesystem_cause = None;
	let mut probe_cause = None;
	let recognizer = FilesystemProbe::new(
		Filesystem::Apfs,
		partition,
		|window: &mut dyn FilesystemSource| {
			probe(window.len(), |offset, output| {
				window.read_exact_at(offset, output)
			})
			.map_err(|error| {
				let kind = error.kind();
				probe_cause = Some(Box::new(error) as Box<dyn Error>);
				classify(kind)
			})
		},
	);
	let checker = |window: &mut dyn FilesystemSource| {
		check(
			window.len(),
			|offset, output| window.read_exact_at(offset, output),
			selector,
			xid,
			data,
			|| false,
		)
		.map(evidence)
		.map_err(|error| {
			let kind = error.kind();
			let candidates = error.selection_candidates();
			filesystem_cause = Some(Box::new(error) as Box<dyn Error>);
			candidates.map_or_else(
				|| classify(kind),
				|candidates| FilesystemCheckError::SelectionRequired { candidates },
			)
		})
	};
	let filesystem = FilesystemOptions::default();
	let report = match encoding {
		Encoding::Raw => {
			dmgez_inspector::verify_raw_filesystem(source, filesystem, recognizer, checker)
		}
		Encoding::Udif(limits) => dmgez_inspector::verify_filesystem_with_probe(
			source, limits, filesystem, recognizer, checker,
		),
	};
	Verification {
		report,
		filesystem_cause: filesystem_cause.or(probe_cause),
	}
}

fn evidence(result: VerificationEvidence) -> FilesystemEvidence {
	let mut rejected_descriptors = 0;
	let mut advertised_xid = None;
	for diagnostic in result.checkpoint_diagnostics {
		match diagnostic {
			CheckpointDiagnostic::Rejected(_) => rejected_descriptors += 1,
			CheckpointDiagnostic::NewerBootstrap { advertised, .. } => {
				advertised_xid = Some(FilesystemCheckpointId(advertised.0));
			}
			_ => {}
		}
	}
	FilesystemEvidence::checkpoint(
		FilesystemScope::ReachableData,
		FilesystemObjectId(result.volume.object_id.0),
		FilesystemCheckpointId(result.volume.checkpoint.0),
		FilesystemUuid(result.volume.uuid),
		Some(result.reachable.payload_bytes_read),
	)
	.with_checkpoint_diagnostics(rejected_descriptors, advertised_xid)
}

fn classify(kind: CheckFailureKind) -> FilesystemCheckError {
	match kind {
		CheckFailureKind::Unsupported => FilesystemCheckError::Unsupported,
		CheckFailureKind::Corrupt => FilesystemCheckError::Corrupt,
		CheckFailureKind::ResourceLimit => FilesystemCheckError::ResourceLimit,
		CheckFailureKind::Selection => FilesystemCheckError::Selection,
		// Inspector preserves the actual error if a source read failed.
		CheckFailureKind::Read => FilesystemCheckError::Unavailable,
		CheckFailureKind::Cancelled => FilesystemCheckError::Cancelled,
		_ => FilesystemCheckError::Unavailable,
	}
}
