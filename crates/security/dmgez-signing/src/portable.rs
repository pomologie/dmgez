use std::{
	fs::File,
	io::{self, Read, Write},
	path::{Path, PathBuf},
};

use dmgez_udif::{
	TRAILER_BYTES, portable_signing_limit, read_embedded_signature,
	reader::{FileSource, Image, Limits},
	signed_signature_range, validate_signature_append, validate_unsigned_signature_fields,
	verify_code_directory_digests,
};
use sha2::{Digest, Sha256};

use crate::SignError;

mod artifact;
mod quarantine;
mod stage;

#[cfg(test)]
mod tests;

use artifact::Artifact;
use stage::Stage;

const HASH_BUFFER_BYTES: usize = 64 << 10;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) struct Published {
	pub(crate) source: PathBuf,
	pub(crate) path: PathBuf,
	pub(crate) unsigned_sha256: String,
	pub(crate) signed_sha256: String,
}

/// Sign a private copy by held handle and publish it after policy verification.
/// The destination must be in a trusted local directory with a stable name.
pub(crate) fn stage_and_sign(
	source: &Path,
	destination: &Path,
	sign: impl FnOnce(&mut File) -> Result<(), SignError>,
	verify: impl FnOnce(&[u8]) -> Result<(), SignError>,
) -> Result<Published, SignError> {
	let source = Artifact::open(source)?;
	let unsigned_trailer = source.trailer()?;
	validate_unsigned_signature_fields(&unsigned_trailer)
		.map_err(|_| SignError::ExpectedUnsigned)?;
	let body_size =
		portable_signing_limit(&unsigned_trailer, source.len()).map_err(integrity_error)?;
	let mut stage = Stage::new(destination)?;
	let quarantine = source.quarantine()?;
	let body_hash = hash_prefix(source.reader(), stage.file_mut(), u64::from(body_size))?;
	stage
		.file_mut()
		.write_all(&unsigned_trailer)
		.map_err(SignError::ImageIo)?;
	stage.write_quarantine(quarantine.as_deref())?;
	stage.sync()?;
	stage.validate()?;
	let prepared = stage.artifact()?;
	check_image(&prepared, ImageKind::Unsigned)?;
	if prepared.trailer()? != unsigned_trailer || prepared.quarantine()? != quarantine {
		return Err(SignError::ImageChanged);
	}
	drop(prepared);
	source.validate()?;
	stage.validate()?;
	sign(stage.file_mut())?;
	stage.sync()?;
	stage.validate()?;

	let signed = stage.artifact()?;
	let signed_trailer = signed.trailer()?;
	let signature = validate_signature_append(
		&unsigned_trailer,
		source.len(),
		&signed_trailer,
		signed.len(),
	)
	.map_err(|_| SignError::ImageChanged)?;
	let mut reader = signed.reader();
	let signed_body = hash_prefix(&mut reader, &mut io::sink(), signature.start)?;
	if body_hash.clone().finalize() != signed_body.clone().finalize() {
		return Err(SignError::ImageChanged);
	}
	let frame = read_embedded_signature(&mut reader, signature.end - signature.start)
		.map_err(integrity_error)?;
	verify_frame(
		&frame,
		&signed_trailer,
		signature.start,
		&signed_body,
		verify,
	)?;
	if source.quarantine()? != quarantine || signed.quarantine()? != quarantine {
		return Err(SignError::ImageChanged);
	}
	drop(reader);
	drop(signed);
	let path = stage.publish()?;
	Ok(Published {
		source: source.path().to_path_buf(),
		path,
		unsigned_sha256: finish_hash(body_hash, &[&unsigned_trailer]),
		signed_sha256: finish_hash(signed_body, &[&frame, &signed_trailer]),
	})
}

/// Verify a bounded, independently opened image using the supplied signature policy.
pub(crate) fn verify_signed(
	path: &Path,
	verify: impl FnOnce(&[u8]) -> Result<(), SignError>,
) -> Result<(), SignError> {
	let artifact = Artifact::open(path)?;
	let trailer = artifact.trailer()?;
	if validate_unsigned_signature_fields(&trailer).is_ok() {
		check_image(&artifact, ImageKind::Unsigned)?;
		artifact.validate()?;
		return Err(SignError::Unsigned);
	}
	let signature = signed_signature_range(&trailer, artifact.len()).map_err(integrity_error)?;
	let mut reader = artifact.reader();
	let body = hash_prefix(&mut reader, &mut io::sink(), signature.start)?;
	let frame = read_embedded_signature(&mut reader, signature.end - signature.start)
		.map_err(integrity_error)?;
	verify_frame(&frame, &trailer, signature.start, &body, verify)?;
	check_image(&artifact, ImageKind::Signed)?;
	artifact.validate()
}

fn verify_frame(
	frame: &[u8],
	trailer: &[u8; TRAILER_BYTES],
	body_size: u64,
	body: &Sha256,
	verify: impl FnOnce(&[u8]) -> Result<(), SignError>,
) -> Result<(), SignError> {
	let digest = body.clone().finalize().into();
	verify_code_directory_digests(frame, trailer, body_size, &digest).map_err(integrity_error)?;
	verify(frame)
}

enum ImageKind {
	Unsigned,
	Signed,
}

fn check_image(artifact: &Artifact, kind: ImageKind) -> Result<(), SignError> {
	let file = artifact.file().try_clone().map_err(SignError::ImageIo)?;
	let source = FileSource::new(file).map_err(integrity_error)?;
	let mut image = match kind {
		ImageKind::Unsigned => Image::open(source, Limits::default()),
		ImageKind::Signed => Image::open_release(source, Limits::default()),
	}
	.map_err(integrity_error)?;
	image.verify().map_err(integrity_error)?;
	artifact.validate()
}

fn hash_prefix(
	mut reader: impl Read,
	mut output: impl Write,
	mut remaining: u64,
) -> Result<Sha256, SignError> {
	let mut hash = Sha256::new();
	let mut buffer = [0; HASH_BUFFER_BYTES];
	while remaining != 0 {
		let length = remaining.min(HASH_BUFFER_BYTES as u64) as usize;
		let chunk = &mut buffer[..length];
		reader.read_exact(chunk).map_err(SignError::ImageIo)?;
		output.write_all(chunk).map_err(SignError::ImageIo)?;
		hash.update(chunk);
		remaining -= length as u64;
	}
	Ok(hash)
}

fn finish_hash(mut hash: Sha256, suffixes: &[&[u8]]) -> String {
	for suffix in suffixes {
		hash.update(suffix);
	}
	let digest = hash.finalize();
	let mut output = String::with_capacity(digest.len() * 2);
	for byte in digest {
		output.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
		output.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
	}
	output
}

fn integrity_error(error: impl std::error::Error + Send + Sync + 'static) -> SignError {
	SignError::ImageIntegrity(Box::new(error))
}
