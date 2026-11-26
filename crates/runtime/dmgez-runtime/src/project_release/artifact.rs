use std::{
	fmt::Write as _,
	fs::{self, File, Metadata},
	io::{self, Read},
	path::Path,
};

use dmgez_core::{
	output::{StagedReader, publish_new},
	source::CapturedFile,
};
use dmgez_signing::release_plan::ImageIdentity;
use dmgez_udif::reader::{FileSource, Image, Limits, ReadAt};
use sha2::{Digest, Sha256};

pub(super) use crate::staged_image::verify_stage;
use crate::staged_image::{StagedImage, invalid, verify_payload};

#[cfg(test)]
mod tests;

const HASH_BUFFER_BYTES: usize = 64 << 10;
const SHA256_HEX_BYTES: usize = 64;
const IMAGE_IDENTITY_CHANGED: &str = "the staged image identity changed";

/// Bind an observation to every byte of a held, finite release file.
pub(super) fn observe(path: &Path) -> io::Result<ImageIdentity> {
	observe_file(open_regular(path)?)
}

/// Record the identity of the same held file the builder verified.
pub(super) fn observe_stage(
	mut reader: StagedReader<'_>,
	physical_bytes: u64,
) -> io::Result<ImageIdentity> {
	let before = reader.metadata()?;
	if before.len() != physical_bytes {
		return Err(invalid(IMAGE_IDENTITY_CHANGED));
	}
	let identity = hash_image(&mut reader, physical_bytes)?;
	if !same_entry(&before, &reader.metadata()?) {
		return Err(invalid(IMAGE_IDENTITY_CHANGED));
	}
	Ok(identity)
}

pub(super) fn observe_file(file: File) -> io::Result<ImageIdentity> {
	let source = CapturedFile::new(file).map_err(io::Error::other)?;
	let identity = hash_image(source.reader(), source.len())?;
	source.validate().map_err(io::Error::other)?;
	Ok(identity)
}

/// Check the UDIF structure and payload, returning the identity of that handle.
pub(super) fn verify(path: &Path) -> io::Result<ImageIdentity> {
	let file = open_regular(path)?;
	let view = FileSource::new(file.try_clone()?).map_err(invalid)?;
	let source = CapturedFile::new(file).map_err(io::Error::other)?;
	verify_container(view)?;
	let identity = hash_image(source.reader(), source.len())?;
	source.validate().map_err(io::Error::other)?;
	Ok(identity)
}

/// Publish a synced, verified copy whose bytes match the admitted identity.
pub(super) fn publish(source: &Path, expected: &ImageIdentity, output: &Path) -> io::Result<()> {
	let source = CapturedFile::new(open_regular(source)?).map_err(io::Error::other)?;
	if source.len() != expected.bytes() {
		return Err(invalid(IMAGE_IDENTITY_CHANGED));
	}
	check_size(source.len())?;
	publish_new(
		output,
		|writer| {
			if io::copy(&mut source.reader(), writer)? != expected.bytes() {
				return Err(invalid(IMAGE_IDENTITY_CHANGED));
			}
			Ok(())
		},
		|reader, &()| {
			let stage = StagedImage::new(reader)?;
			let identity = stage.read_from_start(|reader| hash_image(reader, stage.len()))?;
			if &identity != expected {
				return Err(invalid(IMAGE_IDENTITY_CHANGED));
			}
			verify_container(stage)?;
			source.validate().map_err(io::Error::other)
		},
	)
	.map_err(io::Error::other)
}

fn verify_container(source: impl ReadAt) -> io::Result<()> {
	let mut image = Image::open_release(source, Limits::default()).map_err(invalid)?;
	verify_payload(&mut image)
}

pub(super) fn open_regular(path: &Path) -> io::Result<File> {
	let path_metadata = fs::symlink_metadata(path)?;
	if !path_metadata.is_file() {
		return Err(invalid("release artifact must be a regular file"));
	}
	let file = File::open(path)?;
	revalidate_opened_entry(path, &path_metadata, &file)?;
	Ok(file)
}

fn revalidate_opened_entry(path: &Path, before: &Metadata, file: &File) -> io::Result<()> {
	let opened = file.metadata()?;
	if !same_entry(before, &opened) || !same_entry(&opened, &fs::symlink_metadata(path)?) {
		return Err(invalid(IMAGE_IDENTITY_CHANGED));
	}
	Ok(())
}

fn same_entry(left: &Metadata, right: &Metadata) -> bool {
	#[cfg(unix)]
	{
		use std::os::unix::fs::MetadataExt as _;

		if left.dev() != right.dev() || left.ino() != right.ino() {
			return false;
		}
	}
	right.is_file() && left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn hash_image(mut reader: impl Read, bytes: u64) -> io::Result<ImageIdentity> {
	check_size(bytes)?;
	let mut remaining = bytes;
	let mut hash = Sha256::new();
	let mut buffer = [0; HASH_BUFFER_BYTES];
	while remaining != 0 {
		let count = remaining.min(HASH_BUFFER_BYTES as u64) as usize;
		reader.read_exact(&mut buffer[..count])?;
		hash.update(&buffer[..count]);
		remaining -= count as u64;
	}
	let mut digest = String::with_capacity(SHA256_HEX_BYTES);
	for byte in hash.finalize() {
		write!(digest, "{byte:02x}").map_err(io::Error::other)?;
	}
	ImageIdentity::new(&digest, bytes).map_err(invalid)
}

fn check_size(bytes: u64) -> io::Result<()> {
	if bytes == 0 || bytes > Limits::default().max_physical_bytes {
		return Err(invalid("release image size is outside the admitted bounds"));
	}
	Ok(())
}
