use std::{error::Error, num::NonZeroU32};

use dmgez_hfsplus::CheckFailureKind;
use dmgez_inspector::{
	FilesystemCheckError, FilesystemEvidence, FilesystemOptions, FilesystemProbe, FilesystemSource,
};
use dmgez_udif::reader::{Limits, ReadAt};
use dmgez_vfs::Filesystem;

use super::Verification;

pub(super) fn verify(
	source: impl ReadAt,
	partition: Option<NonZeroU32>,
	max_payload_bytes: u64,
) -> Verification {
	let options = FilesystemOptions::default();
	let limits = Limits::default();
	let mut cause = None;
	let check = |window: &mut dyn FilesystemSource| {
		dmgez_hfsplus::check_reachable_data(window.len(), max_payload_bytes, |offset, output| {
			window.read_exact_at(offset, output)
		})
		.map(|report| FilesystemEvidence::stored_window(report.stored_bytes.0))
		.map_err(|error| {
			let kind = error.kind();
			cause = Some(Box::new(error) as Box<dyn Error>);
			classify(kind)
		})
	};
	let report = if let Some(slot) = partition {
		let probe = FilesystemProbe::new(
			Filesystem::HfsPlus,
			Some(slot),
			|_: &mut dyn FilesystemSource| Ok(None),
		);
		dmgez_inspector::verify_filesystem_with_probe(source, limits, options, probe, check)
	} else {
		dmgez_inspector::verify_filesystem(source, limits, options, check)
	};
	Verification {
		report,
		filesystem_cause: cause,
	}
}

fn classify(kind: CheckFailureKind) -> FilesystemCheckError {
	match kind {
		CheckFailureKind::Unsupported => FilesystemCheckError::Unsupported,
		CheckFailureKind::Corrupt => FilesystemCheckError::Corrupt,
		CheckFailureKind::ResourceLimit => FilesystemCheckError::ResourceLimit,
		// The inspector records the first failed window read and
		// preserves its more specific category.
		CheckFailureKind::Read => FilesystemCheckError::Unavailable,
	}
}
