use dmgez_hfsplus_verify::CheckFailureKind;
use dmgez_inspector::{
	FilesystemCheckError, FilesystemEvidence, FilesystemOptions, FilesystemScope,
	VerificationReport,
};
use dmgez_udif::reader::{FileSource, Limits};

use crate::report_field;

const FILESYSTEM_SCOPE_KEY: &str = "filesystem_scope";
const REACHABLE_STRUCTURE: &str = "reachable_structure";

pub(super) fn verify(source: FileSource) -> VerificationReport {
	dmgez_inspector::verify_filesystem(
		source,
		Limits::default(),
		FilesystemOptions::default(),
		|window| {
			dmgez_hfsplus_verify::check(window.len(), |offset, output| {
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
		},
	)
}

pub(super) fn show_coverage(report: &VerificationReport) {
	if let Some(check) = &report.filesystem_check
		&& check.evidence.as_ref().map(|evidence| evidence.scope)
			== Some(FilesystemScope::ReachableStructure)
	{
		report_field(FILESYSTEM_SCOPE_KEY, REACHABLE_STRUCTURE);
	}
}
