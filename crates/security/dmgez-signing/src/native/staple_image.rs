use std::{io, path::Path};

use crate::notarization::{AcceptedImage, StapleReport};
use crate::{ArtifactKind, ArtifactProblem, PolicyAssessment, ResolvedBackend, SignError};

use super::{
	artifact::Artifact,
	disk::image,
	notary_tool, policy,
	stage::{self, Stage},
	stapler,
};

mod inspection;
mod snapshot;

pub(crate) use inspection::check_ticket;

#[cfg(test)]
mod tests;

const REPORT_VERSION: u16 = 1;

#[derive(Debug, thiserror::Error)]
enum Error {
	#[error("the input does not match the accepted notarization record")]
	DigestMismatch,
	#[error("the input already contains a stapled ticket")]
	AlreadyStapled,
	#[error("the staged image size or embedded signature exceeds the stapling limit")]
	Size,
	#[error("could not prepare a private writable stage")]
	Stage(#[from] stage::Error),
	#[error("the staging directory must have no access control list")]
	StageAcl,
	#[error("could not check staging directory access controls")]
	StagePrivacy,
	#[error("could not read or revalidate the disk image")]
	Artifact(#[from] crate::Error),
	#[error("could not read or write disk image bytes")]
	Io(#[from] io::Error),
	#[error("invalid signed UDIF layout or signature frame")]
	Format(#[from] dmgez_udif::Error),
	#[error("stapling changed the image body or quarantine state")]
	Changed,
	#[error("the Developer ID signature or native image checksum did not verify")]
	Verify(#[from] SignError),
	#[error("the native stapling or ticket validation step failed")]
	Tool(#[from] stapler::Error),
}

pub(crate) fn staple(
	source: &Path,
	output: &Path,
	expected: AcceptedImage,
) -> Result<StapleReport, crate::notarization::Error> {
	stage_and_staple(source, output, expected)
		.map_err(|source| crate::notarization::Error::Staple(Box::new(source)))
}

fn stage_and_staple(
	source: &Path,
	output: &Path,
	expected: AcceptedImage,
) -> Result<StapleReport, Error> {
	let source = open_image(source)?;
	let verify_image = |image: &Artifact| {
		image.validate()?;
		policy::verify_notarization_image(image.path())?;
		image::verify_checksum(image)?;
		image.validate()?;
		Ok(())
	};
	transact(
		&source,
		output,
		expected,
		verify_image,
		|path| stapler::Tool::new()?.staple(path).map_err(Error::from),
		|path| stapler::Tool::new()?.validate(path).map_err(Error::from),
	)
}

fn transact(
	source: &Artifact,
	output: &Path,
	expected: AcceptedImage,
	verify_image: impl Fn(&Artifact) -> Result<(), Error>,
	attach: impl FnOnce(&Path) -> Result<(), Error>,
	validate_ticket: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<StapleReport, Error> {
	if source.len() != expected.bytes {
		return Err(Error::DigestMismatch);
	}
	let stage = Stage::new(output)?;
	let directory = stage.path().parent().ok_or(Error::StagePrivacy)?;
	if notary_tool::has_acl(directory).map_err(|_| Error::StagePrivacy)? {
		return Err(Error::StageAcl);
	}
	let mut file = stage.create_image()?;
	let original = snapshot::Input::copy(source, &mut file)?;
	if !original.sha256.eq_ignore_ascii_case(&expected.digest) {
		return Err(Error::DigestMismatch);
	}

	stage.validate_image(&file)?;
	let prepared = open_image(stage.path())?;
	verify_image(&prepared)?;
	prepared.validate()?;
	stage.validate_image(&file)?;
	source.validate()?;
	drop(file);
	drop(prepared);
	attach(stage.path())?;

	stage.validate()?;
	let stapled = open_image(stage.path())?;
	stage.validate_image(stapled.file())?;
	let output_sha256 = original.verify_ticket_addition(&stapled)?;
	verify_image(&stapled)?;
	validate_ticket(stapled.path())?;
	stapled.validate()?;
	source.validate()?;
	let published = stage.publish(stapled.file())?;

	Ok(StapleReport {
		schema_version: REPORT_VERSION,
		backend: ResolvedBackend::Native,
		submission_id: expected.id,
		source: source.path().to_string_lossy().into_owned(),
		output: published.to_string_lossy().into_owned(),
		input_sha256: original.sha256,
		output_sha256,
		ticket_validated: true,
		assessment: PolicyAssessment::NotPerformed,
	})
}

fn open_image(path: &Path) -> Result<Artifact, Error> {
	let artifact = Artifact::open(path)?;
	if artifact.kind() != ArtifactKind::DiskImage {
		return Err(artifact.invalid(ArtifactProblem::Kind).into());
	}
	if artifact.len() > image::MAX_SIGNED_BYTES {
		return Err(Error::Size);
	}
	Ok(artifact)
}
