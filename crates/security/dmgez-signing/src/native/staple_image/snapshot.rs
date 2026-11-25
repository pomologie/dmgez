use std::{
	fs::File,
	io::{self, Read, Seek, Write},
	ops::Range,
};

use dmgez_udif::{
	TRAILER_BYTES, has_stapled_ticket, signed_signature_range, validate_staple_layout,
	validate_ticket_addition,
};
use sha2::{Digest, Sha256};

use crate::native::disk::image::{
	MAX_SIGNATURE_BYTES, copy_to, encode_hex, read_quarantine, read_trailer, write_quarantine,
};

use super::{Artifact, Error};

pub(super) struct Input {
	pub(super) sha256: String,
	length: u64,
	trailer: [u8; TRAILER_BYTES],
	body: [u8; 32],
	signature: Vec<u8>,
	quarantine: Option<Vec<u8>>,
}

impl Input {
	pub(super) fn copy(image: &Artifact, stage: &mut File) -> Result<Self, Error> {
		let trailer = read_trailer(image.file())?;
		let range = signed_signature_range(&trailer, image.len())?;
		let mut signature = signature_buffer(&range)?;
		let mut input = image.file();
		input.rewind()?;
		let mut hash = Sha256::new();
		copy_to(&mut input, stage, range.start, &mut hash)?;
		let body = hash.clone().finalize().into();
		input.read_exact(&mut signature)?;
		if has_stapled_ticket(&signature)? {
			return Err(Error::AlreadyStapled);
		}
		stage.write_all(&signature)?;
		stage.write_all(&trailer)?;
		hash.update(&signature);
		hash.update(trailer);
		let quarantine = read_quarantine(image.file())?;
		write_quarantine(stage, quarantine.as_deref())?;
		stage.sync_all()?;
		image.validate()?;
		Ok(Self {
			sha256: encode_hex(&hash.finalize()),
			length: image.len(),
			trailer,
			body,
			signature,
			quarantine,
		})
	}

	pub(super) fn verify_ticket_addition(&self, image: &Artifact) -> Result<String, Error> {
		let trailer = read_trailer(image.file())?;
		validate_staple_layout(&self.trailer, self.length, &trailer, image.len())?;
		let range = signed_signature_range(&trailer, image.len())?;
		let mut signature = signature_buffer(&range)?;
		let mut input = image.file();
		input.rewind()?;
		let mut hash = Sha256::new();
		copy_to(&mut input, &mut io::sink(), range.start, &mut hash)?;
		let body: [u8; 32] = hash.clone().finalize().into();
		if body != self.body || read_quarantine(image.file())? != self.quarantine {
			return Err(Error::Changed);
		}
		input.read_exact(&mut signature)?;
		validate_ticket_addition(&self.signature, &signature)?;
		hash.update(&signature);
		hash.update(trailer);
		image.validate()?;
		Ok(encode_hex(&hash.finalize()))
	}
}

fn signature_buffer(range: &Range<u64>) -> Result<Vec<u8>, Error> {
	let size = range.end - range.start;
	if size > MAX_SIGNATURE_BYTES {
		return Err(Error::Size);
	}
	let size = usize::try_from(size).map_err(|_| Error::Size)?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(size).map_err(io::Error::other)?;
	bytes.resize(size, 0);
	Ok(bytes)
}
