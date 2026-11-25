use std::path::Path;

use crate::{
	ArtifactKind, ArtifactProblem, Backend, DiskSigningReport, Operation, SignError, Status,
};

use super::{artifact::Artifact, identity, run, stage::Stage};

pub(super) mod image;
mod policy;

#[cfg(test)]
mod tests;

use policy::DeveloperId;

const SIGNING_REPORT_VERSION: u16 = 1;

pub(crate) fn sign(
	source: &Path,
	destination: &Path,
	identity: &str,
	identifier: &str,
) -> Result<DiskSigningReport, SignError> {
	let developer = DeveloperId::new(identity, identifier)?;
	let source = open_unsigned(source)?;
	if !identity::developer_id_available(developer.fingerprint())
		.map_err(|source| SignError::IdentityLookup(Box::new(source)))?
	{
		return Err(SignError::IdentityUnavailable);
	}

	let published = stage_and_sign(
		&source,
		destination,
		|path| developer.sign(path),
		|artifact| developer.verify(artifact),
	)?;
	Ok(DiskSigningReport {
		schema_version: SIGNING_REPORT_VERSION,
		backend: crate::ResolvedBackend::Native,
		source: source.path().to_string_lossy().into_owned(),
		output: published.path.to_string_lossy().into_owned(),
		identity: developer.fingerprint().to_owned(),
		identifier: identifier.to_owned(),
		unsigned_sha256: published.unsigned_sha256,
		signed_sha256: published.signed_sha256,
		assessment: crate::PolicyAssessment::NotPerformed,
	})
}

pub(crate) fn verify(path: &Path, identity: &str, identifier: &str) -> Result<(), SignError> {
	let developer = DeveloperId::new(identity, identifier)?;
	let artifact = open_image(path)?;
	let trailer = image::read_trailer(artifact.file())?;
	let signature = dmgez_udif::signed_signature_range(&trailer, artifact.len())
		.map_err(|error| SignError::ImageIntegrity(Box::new(error)))?;
	if signature.end - signature.start > image::MAX_SIGNATURE_BYTES {
		return Err(SignError::ImageSize);
	}
	developer.verify(&artifact)?;
	image::verify_checksum(&artifact)?;
	artifact.validate()?;
	Ok(())
}

fn open_image(path: &Path) -> Result<Artifact, SignError> {
	let artifact = Artifact::open(path)?;
	if artifact.kind() != ArtifactKind::DiskImage {
		return Err(artifact.invalid(ArtifactProblem::Kind).into());
	}
	Ok(artifact)
}

fn open_unsigned(path: &Path) -> Result<Artifact, SignError> {
	let artifact = open_image(path)?;
	image::validate_unsigned(&artifact)?;
	let report = inspect(&artifact)?;
	match report.status {
		Status::Unsigned => Ok(artifact),
		Status::Unavailable => Err(SignError::BackendUnavailable {
			backend: Backend::Native,
		}),
		_ => Err(SignError::ExpectedUnsigned),
	}
}

fn inspect(artifact: &Artifact) -> Result<crate::Report, SignError> {
	let mut report =
		crate::signature::report_base(artifact.path(), Operation::Inspect, Backend::Native);
	run(artifact, Operation::Inspect, &mut report)?;
	Ok(report)
}

struct Published {
	path: std::path::PathBuf,
	unsigned_sha256: String,
	signed_sha256: String,
}

fn stage_and_sign(
	source: &Artifact,
	destination: &Path,
	sign: impl FnOnce(&Path) -> Result<(), SignError>,
	verify: impl FnOnce(&Artifact) -> Result<(), SignError>,
) -> Result<Published, SignError> {
	let stage = stage_result(Stage::new(destination))?;
	let mut stage_file = stage_result(stage.create_image())?;
	let unsigned = image::copy_unsigned(source, &mut stage_file)?;

	stage_result(stage.validate_image(&stage_file))?;
	let prepared = open_unsigned(stage.path())?;
	image::verify_checksum(&prepared)?;
	// Reconfirm the staged file and original snapshot before using the key.
	prepared.validate()?;
	stage_result(stage.validate_image(&stage_file))?;
	source.validate()?;
	drop(stage_file);
	drop(prepared);
	sign(stage.path())?;

	stage_result(stage.validate())?;
	let signed = Artifact::open(stage.path())?;
	stage_result(stage.validate_image(signed.file()))?;
	let signed_sha256 = unsigned.check_signed(&signed)?;
	verify(&signed)?;
	image::verify_checksum(&signed)?;
	signed.validate()?;
	source.validate()?;
	let path = stage_result(stage.publish(signed.file()))?;
	Ok(Published {
		path,
		unsigned_sha256: unsigned.sha256,
		signed_sha256,
	})
}

fn stage_result<T>(result: Result<T, super::stage::Error>) -> Result<T, SignError> {
	result.map_err(|source| SignError::Stage(Box::new(source)))
}
