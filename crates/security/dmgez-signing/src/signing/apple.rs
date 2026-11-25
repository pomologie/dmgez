use std::path::Path;

use apple_codesign::dmg::{DmgSigner, DmgSigningSettings};

use crate::{
	ArtifactKind, Backend, DiskSigningMode, DiskSigningReport, Error, Operation, PolicyAssessment,
	Report, ResolvedBackend, SignError, Status, is_valid_disk_identifier, portable,
	signature::report_base,
};

use super::SIGNING_REPORT_VERSION;

pub(super) fn sign(
	source: &Path,
	destination: &Path,
	identifier: &str,
) -> Result<DiskSigningReport, SignError> {
	if !is_valid_disk_identifier(identifier) {
		return Err(SignError::Identifier);
	}
	let settings = DmgSigningSettings {
		identifier: Some(identifier),
		..DmgSigningSettings::default()
	};
	let published = portable::stage_and_sign(
		source,
		destination,
		|stage| {
			DmgSigner::default()
				.sign_file(&settings, stage)
				.map_err(|_| SignError::AppleCodesign)
		},
		|signature| check_ad_hoc(signature, Some(identifier)),
	)?;
	Ok(DiskSigningReport {
		schema_version: SIGNING_REPORT_VERSION,
		backend: ResolvedBackend::Rust,
		mode: DiskSigningMode::AdHoc,
		source: published.source.to_string_lossy().into_owned(),
		output: published.path.to_string_lossy().into_owned(),
		identity: None,
		identifier: identifier.to_owned(),
		unsigned_sha256: published.unsigned_sha256,
		signed_sha256: published.signed_sha256,
		assessment: PolicyAssessment::NotPerformed,
	})
}

pub(super) fn verify(path: &Path, identifier: &str) -> Result<(), SignError> {
	if !is_valid_disk_identifier(identifier) {
		return Err(SignError::Identifier);
	}
	portable::verify_signed(path, |signature| check_ad_hoc(signature, Some(identifier)))
}

pub(crate) fn check(path: &Path, operation: Operation, backend: Backend) -> Result<Report, Error> {
	let mut report = report_base(path, operation, backend);
	report.backend = Some(ResolvedBackend::Rust);
	report.kind = Some(ArtifactKind::DiskImage);
	let result = portable::verify_signed(path, |signature| check_ad_hoc(signature, None));
	match result {
		Ok(()) => {
			report.status = match operation {
				Operation::Inspect => Status::Present,
				Operation::Verify => Status::Verified,
			};
			report.diagnostics = Some("ad hoc CodeDirectory integrity verified".into());
		}
		Err(SignError::Unsigned) => report.status = Status::Unsigned,
		Err(SignError::AdHocProfile) => {
			report.status = Status::Unavailable;
			report.diagnostics = Some("Rust verification supports ad hoc UDIF signatures".into());
		}
		Err(error @ (SignError::ImageChecksum | SignError::ImageIntegrity(_))) => {
			report.status = Status::Failed;
			report.diagnostics = Some(error.to_string());
		}
		Err(error) => return Err(Error::Rust(Box::new(error))),
	}
	Ok(report)
}

fn check_ad_hoc(signature: &[u8], expected_identifier: Option<&str>) -> Result<(), SignError> {
	let directory = dmgez_udif::embedded_code_directory(signature)
		.map_err(|error| SignError::ImageIntegrity(Box::new(error)))?;
	if !directory.ad_hoc || directory.has_cms_signature || directory.has_ticket {
		return Err(SignError::AdHocProfile);
	}
	let identifier = std::str::from_utf8(directory.identifier)
		.ok()
		.filter(|identifier| is_valid_disk_identifier(identifier))
		.ok_or(SignError::AdHocProfile)?;
	if expected_identifier.is_some_and(|expected| expected != identifier) {
		return Err(SignError::AdHocIdentifier);
	}
	Ok(())
}
