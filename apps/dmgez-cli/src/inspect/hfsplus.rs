use std::{error::Error, num::NonZeroU32};

use dmgez_hfsplus_read::CheckFailureKind;
use dmgez_inspector::{
	FilesystemCheckError, FilesystemEvidence, FilesystemOptions, FilesystemProbe, FilesystemSource,
	VerificationReport,
};
use dmgez_udif::reader::{FileSource, Limits};
use dmgez_vfs::Filesystem;

pub(super) fn verify(
	source: FileSource,
	partition: Option<NonZeroU32>,
	max_payload_bytes: u64,
) -> (VerificationReport, Option<Box<dyn Error>>) {
	let options = FilesystemOptions::default();
	let limits = Limits::default();
	let mut cause = None;
	let check = |window: &mut dyn FilesystemSource| {
		dmgez_hfsplus_read::check_reachable_data(
			window.len(),
			max_payload_bytes,
			|offset, output| window.read_exact_at(offset, output),
		)
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
	(report, cause)
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
