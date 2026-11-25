use std::num::NonZeroU32;

use dmgez_hfsplus_read::CheckFailureKind;
use dmgez_inspector::{
	FilesystemCheckError, FilesystemEvidence, FilesystemOptions, FilesystemProbe, FilesystemScope,
	FilesystemSource, VerificationReport,
};
use dmgez_udif::reader::{FileSource, Limits};
use dmgez_vfs::Filesystem;

pub(super) fn verify(source: FileSource, partition: Option<NonZeroU32>) -> VerificationReport {
	let options = FilesystemOptions::default();
	let limits = Limits::default();
	if let Some(slot) = partition {
		let probe = FilesystemProbe::new(
			Filesystem::HfsPlus,
			Some(slot),
			|_: &mut dyn FilesystemSource| Ok(None),
		);
		return dmgez_inspector::verify_filesystem_with_probe(
			source, limits, options, probe, check,
		);
	}
	dmgez_inspector::verify_filesystem(source, limits, options, check)
}

fn check(window: &mut dyn FilesystemSource) -> Result<FilesystemEvidence, FilesystemCheckError> {
	dmgez_hfsplus_read::check(window.len(), |offset, output| {
		window.read_exact_at(offset, output)
	})
	.map(|_| FilesystemEvidence::window(FilesystemScope::ReachableStructure))
	.map_err(|error| match error.kind() {
		CheckFailureKind::Unsupported => FilesystemCheckError::Unsupported,
		CheckFailureKind::Corrupt => FilesystemCheckError::Corrupt,
		CheckFailureKind::ResourceLimit => FilesystemCheckError::ResourceLimit,
		// The inspector records the first failed window read and
		// preserves its more specific category.
		CheckFailureKind::Read => FilesystemCheckError::Unavailable,
	})
}
