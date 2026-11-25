use std::{
	fs::{OpenOptions, Permissions},
	io::{self, Read, Seek, SeekFrom},
	os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
	path::Path,
};

use dmgez_udif::{TRAILER_BYTES, validate_unsigned_signature_fields};
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

use crate::{ArtifactKind, ArtifactProblem, SignError};

use super::{
	artifact::Artifact,
	disk::image::{MAX_SIGNED_BYTES, copy_to, encode_hex},
	policy::verify_notarization_image,
};

#[cfg(test)]
mod tests;

const STAGE_PREFIX: &str = ".dmgez-notary-";
const STAGE_DIRECTORY_MODE: u32 = 0o700;
const STAGE_IMAGE_MODE: u32 = 0o400;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
	#[error("could not open or revalidate the disk image")]
	Artifact(#[from] crate::Error),
	#[error("the disk image exceeds the notarization staging limit")]
	Size,
	#[error("the disk image does not contain an embedded signature")]
	Unsigned,
	#[error("could not stage the exact disk image bytes")]
	Stage(#[source] io::Error),
	#[error("the staged disk image changed")]
	Changed,
	#[error("the disk image needs a valid Developer ID Application signature and secure timestamp")]
	Signature(#[from] SignError),
}

/// Stable private copy of the bytes that will be handed to the Apple tool.
pub(crate) struct PreparedArtifact {
	image: Artifact,
	sha256: String,
	_stage: TempDir,
}

impl PreparedArtifact {
	pub(crate) fn new(source: &Path) -> Result<Self, Error> {
		let source = open_image(source)?;
		if source.len() > MAX_SIGNED_BYTES {
			return Err(Error::Size);
		}
		// Skip the copy for clearly unsigned images.
		let mut input = source.file();
		let mut trailer = [0; TRAILER_BYTES];
		input
			.seek(SeekFrom::End(-(TRAILER_BYTES as i64)))
			.map_err(Error::Stage)?;
		input.read_exact(&mut trailer).map_err(Error::Stage)?;
		source.validate()?;
		if validate_unsigned_signature_fields(&trailer).is_ok() {
			return Err(Error::Unsigned);
		}
		let prepared = Self::copy(&source)?;
		prepared.validate()?;
		verify_notarization_image(prepared.path())?;
		prepared.validate()?;
		Ok(prepared)
	}

	fn copy(source: &Artifact) -> Result<Self, Error> {
		let parent = std::env::temp_dir().canonicalize().map_err(Error::Stage)?;
		let stage = Builder::new()
			.prefix(STAGE_PREFIX)
			.permissions(Permissions::from_mode(STAGE_DIRECTORY_MODE))
			.tempdir_in(parent)
			.map_err(Error::Stage)?;
		let basename = source.path().file_name().ok_or(Error::Changed)?;
		let path = stage.path().join(basename);
		let mut file = OpenOptions::new()
			.create_new(true)
			.write(true)
			.mode(STAGE_IMAGE_MODE)
			.open(&path)
			.map_err(Error::Stage)?;
		let mut input = source.file();
		input.rewind().map_err(Error::Stage)?;
		let mut hash = Sha256::new();
		copy_to(&mut input, &mut file, source.len(), &mut hash).map_err(Error::Stage)?;
		let sha256 = encode_hex(&hash.finalize());
		file.sync_all().map_err(Error::Stage)?;
		source.validate()?;
		let image = open_image(&path)?;
		let written = file.metadata().map_err(Error::Stage)?;
		let opened = image.file().metadata().map_err(Error::Stage)?;
		if image.path() != path
			|| image.len() != source.len()
			|| written.dev() != opened.dev()
			|| written.ino() != opened.ino()
		{
			return Err(Error::Changed);
		}
		Ok(Self {
			image,
			sha256,
			_stage: stage,
		})
	}

	pub(crate) fn path(&self) -> &Path {
		self.image.path()
	}

	pub(crate) fn sha256(&self) -> &str {
		&self.sha256
	}

	pub(crate) fn len(&self) -> u64 {
		self.image.len()
	}

	pub(crate) fn validate(&self) -> Result<(), Error> {
		match self.image.validate() {
			Err(crate::Error::ArtifactChanged { .. }) => Err(Error::Changed),
			other => other.map_err(Error::Artifact),
		}
	}
}

fn open_image(path: &Path) -> Result<Artifact, Error> {
	let artifact = Artifact::open(path)?;
	if artifact.kind() != ArtifactKind::DiskImage {
		return Err(artifact.invalid(ArtifactProblem::Kind).into());
	}
	Ok(artifact)
}
