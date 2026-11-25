use std::{
	ffi::OsStr,
	fs::File,
	io::{self, Read, Seek, SeekFrom, Write},
};

use dmgez_udif::{TRAILER_BYTES, validate_signature_append, validate_unsigned_signature_fields};
use rustix::{fs, io::Errno};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, SignError};

use super::Artifact;
use crate::native::runner;

const MAX_UNSIGNED_BYTES: u64 = 1 << 40;
const MAX_SIGNATURE_BYTES: u64 = 16 << 20;
#[cfg(feature = "notarization-native")]
pub(in crate::native) const MAX_SIGNED_BYTES: u64 = MAX_UNSIGNED_BYTES + MAX_SIGNATURE_BYTES;
const COPY_BUFFER_BYTES: usize = 64 << 10;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const QUARANTINE: &str = "com.apple.quarantine";
const MAX_QUARANTINE_BYTES: usize = 4 << 10;
const VERIFY_OPTIONS: &[&str] = &["verify", "-nocache", "-plist"];
const CHECKSUM_TYPE: &str = "CRC32";

pub(super) fn validate_unsigned(artifact: &Artifact) -> Result<(), SignError> {
	if artifact.len() > MAX_UNSIGNED_BYTES {
		return Err(SignError::ImageSize);
	}
	let trailer = read_trailer(artifact.file())?;
	validate_unsigned_signature_fields(&trailer).map_err(|_| SignError::ExpectedUnsigned)?;
	artifact.validate()?;
	Ok(())
}

pub(super) struct Unsigned {
	pub(super) sha256: String,
	body_sha256: [u8; 32],
	trailer: [u8; TRAILER_BYTES],
	size: u64,
	quarantine: Option<Vec<u8>>,
}

pub(super) fn copy_unsigned(source: &Artifact, stage: &mut File) -> Result<Unsigned, SignError> {
	let size = source.len();
	let body_len = body_length(size)?;
	let mut input = source.file();
	input.rewind().map_err(SignError::ImageIo)?;
	let mut hash = Sha256::new();
	copy_to(&mut input, stage, body_len, &mut hash).map_err(SignError::ImageIo)?;
	let body_sha256 = hash.clone().finalize().into();
	let mut trailer = [0; TRAILER_BYTES];
	input.read_exact(&mut trailer).map_err(SignError::ImageIo)?;
	stage.write_all(&trailer).map_err(SignError::ImageIo)?;
	hash.update(trailer);

	let quarantine = read_quarantine(source.file())?;
	if let Some(value) = &quarantine {
		fs::fsetxattr(&*stage, QUARANTINE, value, fs::XattrFlags::empty())
			.map_err(|error| SignError::Quarantine(error.into()))?;
	}
	stage.sync_all().map_err(SignError::ImageIo)?;
	source.validate()?;
	Ok(Unsigned {
		sha256: encode_hex(&hash.finalize()),
		body_sha256,
		trailer,
		size,
		quarantine,
	})
}

impl Unsigned {
	pub(super) fn check_signed(&self, signed: &Artifact) -> Result<String, SignError> {
		let signed_trailer = read_trailer(signed.file())?;
		let signature =
			validate_signature_append(&self.trailer, self.size, &signed_trailer, signed.len())
				.map_err(|_| SignError::ImageChanged)?;
		if signature.end - signature.start > MAX_SIGNATURE_BYTES {
			return Err(SignError::ImageChanged);
		}
		let mut input = signed.file();
		input.rewind().map_err(SignError::ImageIo)?;
		let mut hash = Sha256::new();
		copy_to(&mut input, &mut io::sink(), signature.start, &mut hash)
			.map_err(SignError::ImageIo)?;
		let signed_body: [u8; 32] = hash.clone().finalize().into();
		if signed_body != self.body_sha256 || read_quarantine(signed.file())? != self.quarantine {
			return Err(SignError::ImageChanged);
		}
		copy_to(
			&mut input,
			&mut io::sink(),
			signature.end - signature.start,
			&mut hash,
		)
		.map_err(SignError::ImageIo)?;
		hash.update(signed_trailer);
		signed.validate()?;
		Ok(encode_hex(&hash.finalize()))
	}
}

pub(super) fn verify_checksum(artifact: &Artifact) -> Result<(), SignError> {
	let mut arguments: Vec<_> = VERIFY_OPTIONS.iter().map(OsStr::new).collect();
	// hdiutil rejects an end-of-options marker. The captured absolute path
	// cannot be interpreted as a flag, positional URL, or special operand.
	arguments.push(artifact.path().as_os_str());
	artifact.validate()?;
	let output = runner::hdiutil(&arguments).map_err(|source| Error::Tool {
		source: Box::new(source),
	})?;
	artifact.validate()?;
	if !output.status.success() || !matching_crc(&output.stdout) {
		return Err(SignError::ImageChecksum);
	}
	Ok(())
}

#[derive(Deserialize)]
struct Verification {
	calculated: ChecksumRecord,
	expected: ChecksumRecord,
}

#[derive(Deserialize, Eq, PartialEq)]
struct ChecksumRecord {
	#[serde(rename = "Checksum Type")]
	kind: String,
	#[serde(rename = "Checksum Value")]
	value: String,
}

fn matching_crc(report: &[u8]) -> bool {
	let Ok(result) = plist::from_bytes::<Verification>(report) else {
		return false;
	};
	result.expected == result.calculated
		&& result.expected.kind == CHECKSUM_TYPE
		&& result.expected.value.strip_prefix('$').is_some_and(|hex| {
			hex.len() == 2 * size_of::<u32>() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
		})
}

fn read_quarantine(file: &File) -> Result<Option<Vec<u8>>, SignError> {
	let mut value = [0; MAX_QUARANTINE_BYTES];
	match fs::fgetxattr(file, QUARANTINE, &mut value[..]) {
		Ok(size) => Ok(Some(value[..size].to_vec())),
		Err(Errno::NOATTR) => Ok(None),
		Err(error) => Err(SignError::Quarantine(error.into())),
	}
}

fn read_trailer(mut file: &File) -> Result<[u8; TRAILER_BYTES], SignError> {
	let mut trailer = [0; TRAILER_BYTES];
	file.seek(SeekFrom::End(-(TRAILER_BYTES as i64)))
		.map_err(SignError::ImageIo)?;
	file.read_exact(&mut trailer).map_err(SignError::ImageIo)?;
	Ok(trailer)
}

fn body_length(size: u64) -> Result<u64, SignError> {
	size.checked_sub(TRAILER_BYTES as u64)
		.ok_or(SignError::ImageSize)
}

pub(in crate::native) fn encode_hex(digest: &[u8]) -> String {
	let mut text = String::with_capacity(digest.len() * 2);
	for byte in digest {
		text.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
		text.push(char::from(HEX_DIGITS[usize::from(byte & 0xf)]));
	}
	text
}

pub(in crate::native) fn copy_to(
	input: &mut impl Read,
	output: &mut impl Write,
	mut remaining: u64,
	hash: &mut Sha256,
) -> io::Result<()> {
	let mut buffer = [0; COPY_BUFFER_BYTES];
	while remaining != 0 {
		let len = remaining.min(buffer.len() as u64) as usize;
		input.read_exact(&mut buffer[..len])?;
		output.write_all(&buffer[..len])?;
		hash.update(&buffer[..len]);
		remaining -= len as u64;
	}
	Ok(())
}
