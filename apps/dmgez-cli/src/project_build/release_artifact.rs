use std::{
	cell::RefCell,
	fmt::Write as _,
	fs::{self, File, Metadata},
	io::{self, Read, Seek, SeekFrom},
	path::Path,
};

use dmgez_core::{
	output::{StagedReader, publish_new},
	source::CapturedFile,
};
use dmgez_signing::release_plan::ImageIdentity;
use dmgez_udif::{
	reader::{CheckStatus, Checksum, FileSource, Image, Limits, ReadAt},
	writer::WriteReport,
};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

const HASH_BUFFER_BYTES: usize = 64 << 10;
const SHA256_HEX_BYTES: usize = 64;
const IMAGE_IDENTITY_CHANGED: &str = "the staged image identity changed";
const WRITER_REPORT_MISMATCH: &str = "the staged image does not match the writer report";
const CHECKSUMS_MISSING: &str = "the release image is missing required CRC32 checksums";

/// Bind an observation to every byte of a held, finite release file.
pub(super) fn observe(path: &Path) -> io::Result<ImageIdentity> {
	let source = CapturedFile::new(open_regular(path)?).map_err(io::Error::other)?;
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

/// Independently validate the synced unsigned builder stage before it is used.
pub(super) fn verify_stage(reader: StagedReader<'_>, report: &WriteReport) -> io::Result<()> {
	let mut image = Image::open(StagedImage::new(reader)?, Limits::default()).map_err(invalid)?;
	let descriptor = image.descriptor();
	let [table] = descriptor.tables.as_slice() else {
		return Err(invalid(WRITER_REPORT_MISMATCH));
	};
	if descriptor.physical_bytes != report.physical_bytes
		|| descriptor.logical_bytes != report.logical_bytes
		|| descriptor.physical_crc != Checksum::Crc32(report.physical_crc)
		|| descriptor.master_crc != Checksum::Crc32(report.master_crc)
		|| table.checksum != Checksum::Crc32(report.table_crc)
	{
		return Err(invalid(WRITER_REPORT_MISMATCH));
	}
	verify_payload(&mut image)
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
			if &stage.identity()? != expected {
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

fn verify_payload(image: &mut Image<impl ReadAt>) -> io::Result<()> {
	let checked = image.verify().map_err(invalid)?;
	if checked.data_fork != CheckStatus::Passed
		|| checked.master != CheckStatus::Passed
		|| checked
			.tables
			.iter()
			.any(|status| *status != CheckStatus::Passed)
	{
		return Err(invalid(CHECKSUMS_MISSING));
	}
	Ok(())
}

fn open_regular(path: &Path) -> io::Result<File> {
	let path_metadata = fs::symlink_metadata(path)?;
	if !path_metadata.is_file() {
		return Err(invalid("release artifact must be a regular file"));
	}
	let file = File::open(path)?;
	let opened = file.metadata()?;
	if !opened.is_file() || !same_entry(&opened, &fs::symlink_metadata(path)?) {
		return Err(invalid(IMAGE_IDENTITY_CHANGED));
	}
	Ok(file)
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

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, error)
}

struct StagedImage<'file> {
	reader: RefCell<StagedReader<'file>>,
	length: u64,
}

impl<'file> StagedImage<'file> {
	fn new(mut reader: StagedReader<'file>) -> io::Result<Self> {
		let length = reader.seek(SeekFrom::End(0))?;
		Ok(Self {
			reader: RefCell::new(reader),
			length,
		})
	}

	fn identity(&self) -> io::Result<ImageIdentity> {
		let mut reader = self.reader.try_borrow_mut().map_err(io::Error::other)?;
		reader.rewind()?;
		hash_image(&mut *reader, self.length)
	}
}

impl ReadAt for StagedImage<'_> {
	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), dmgez_udif::Error> {
		let len = output.len() as u64;
		if offset.checked_add(len).is_none_or(|end| end > self.length) {
			return Err(dmgez_udif::Error::OutOfRange {
				offset,
				len,
				disk_len: self.length,
			});
		}
		let mut reader = self.reader.try_borrow_mut().map_err(io::Error::other)?;
		reader.seek(SeekFrom::Start(offset))?;
		reader.read_exact(output)?;
		Ok(())
	}
}
