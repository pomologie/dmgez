//! SHA-256 integrity checks for the portable UDIF CodeDirectory profile.

use std::io::{self, Read, Seek, SeekFrom};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
	EmbeddedCodeDirectory, Error as UdifError, MAX_EMBEDDED_SIGNATURE_BYTES, TRAILER_BYTES,
	embedded_code_directory, signed_signature_range, trailer_for_signature_digest,
};

const HASH_BUFFER_BYTES: usize = 64 << 10;

/// Failure to verify the recorded digests of a signed disk image.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SignatureIntegrityError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Format(#[from] UdifError),
	#[error("CodeDirectory length differs from the UDIF body length")]
	BodyLength,
	#[error("UDIF body digest differs from the CodeDirectory")]
	BodyDigest,
	#[error("UDIF trailer digest differs from the CodeDirectory")]
	TrailerDigest,
	#[error("UDIF requirements digest differs from the CodeDirectory")]
	RequirementsDigest,
}

/// Recomputes the signed body, trailer and requirements digests of a UDIF image.
///
/// Supply a stable, complete disk image, such as an immutable staged file.
/// This checks the CodeDirectory's recorded SHA-256 digests. Authenticate the
/// CodeDirectory and evaluate platform trust separately.
///
/// # Errors
///
/// Returns an error for an unsigned or malformed image, an unsupported
/// CodeDirectory profile, a digest mismatch, or an I/O failure.
pub fn verify_signature_integrity(
	image: &mut (impl Read + Seek),
) -> Result<(), SignatureIntegrityError> {
	let length = image.seek(SeekFrom::End(0))?;
	let trailer_offset = length
		.checked_sub(TRAILER_BYTES as u64)
		.ok_or(UdifError::InvalidFormat("short UDIF image"))?;
	image.seek(SeekFrom::Start(trailer_offset))?;
	let mut trailer = [0; TRAILER_BYTES];
	image.read_exact(&mut trailer)?;
	let range = signed_signature_range(&trailer, length)?;
	image.seek(SeekFrom::Start(range.start))?;
	let signature = read_embedded_signature(image, range.end - range.start)?;
	let directory = embedded_code_directory(&signature)?;
	check_metadata(&directory, &trailer, range.start)?;
	image.rewind()?;
	let digest = hash_body(image, range.start)?;
	check_body(&directory, &digest)
}

/// Verifies the CodeDirectory's digests using a previously computed body hash.
///
/// Validate the UDIF signature range before calling. Supply the signature
/// frame, complete signed trailer, signature offset and digest of every image
/// byte preceding the signature, all from the same immutable snapshot.
///
/// # Errors
/// Returns an error for an unsupported or malformed signature frame, a
/// CodeDirectory length that differs from the supplied offset, or a mismatch
/// in the body, trailer or requirements digest.
pub fn verify_code_directory_digests(
	signature: &[u8],
	trailer: &[u8; TRAILER_BYTES],
	body_length: u64,
	body_sha256: &[u8; 32],
) -> Result<(), SignatureIntegrityError> {
	let directory = embedded_code_directory(signature)?;
	check_metadata(&directory, trailer, body_length)?;
	check_body(&directory, body_sha256)
}

fn check_metadata(
	directory: &EmbeddedCodeDirectory<'_>,
	trailer: &[u8; TRAILER_BYTES],
	body_length: u64,
) -> Result<(), SignatureIntegrityError> {
	if directory.signing_limit != body_length {
		return Err(SignatureIntegrityError::BodyLength);
	}
	if Sha256::digest(trailer_for_signature_digest(trailer)).as_slice() != directory.trailer_hash {
		return Err(SignatureIntegrityError::TrailerDigest);
	}
	if Sha256::digest(directory.requirements).as_slice() != directory.requirements_hash {
		return Err(SignatureIntegrityError::RequirementsDigest);
	}
	Ok(())
}

fn check_body(
	directory: &EmbeddedCodeDirectory<'_>,
	body: &[u8; 32],
) -> Result<(), SignatureIntegrityError> {
	if directory.body_hash != body {
		return Err(SignatureIntegrityError::BodyDigest);
	}
	Ok(())
}

/// Reads an embedded signature frame from a reader positioned at its start.
///
/// # Errors
/// Returns an error for an excessive length, allocation failure or failed read.
pub fn read_embedded_signature(image: &mut impl Read, length: u64) -> Result<Vec<u8>, UdifError> {
	let length = usize::try_from(length)
		.ok()
		.filter(|length| *length <= MAX_EMBEDDED_SIGNATURE_BYTES)
		.ok_or(UdifError::ResourceLimit("UDIF embedded signature bytes"))?;
	let mut signature = Vec::new();
	signature
		.try_reserve_exact(length)
		.map_err(io::Error::other)?;
	signature.resize(length, 0);
	image.read_exact(&mut signature)?;
	Ok(signature)
}

fn hash_body(input: &mut impl Read, mut remaining: u64) -> io::Result<[u8; 32]> {
	let mut hash = Sha256::new();
	let mut buffer = [0; HASH_BUFFER_BYTES];
	while remaining != 0 {
		let len = remaining.min(buffer.len() as u64) as usize;
		input.read_exact(&mut buffer[..len])?;
		hash.update(&buffer[..len]);
		remaining -= len as u64;
	}
	Ok(hash.finalize().into())
}
