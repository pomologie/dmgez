use std::{
	cell::RefCell,
	io::{self, Read, Seek, SeekFrom},
};

use dmgez_core::output::StagedReader;
use dmgez_udif::{
	reader::{CheckStatus, Checksum, Image, Limits, ReadAt},
	writer::WriteReport,
};

const WRITER_REPORT_MISMATCH: &str = "the staged image does not match the writer report";
const CHECKSUMS_MISSING: &str = "the staged image is missing required CRC32 checksums";

/// Independently validate the synced unsigned builder stage before it is used.
pub(crate) fn verify_stage(reader: StagedReader<'_>, report: &WriteReport) -> io::Result<()> {
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

pub(crate) fn verify_payload(image: &mut Image<impl ReadAt>) -> io::Result<()> {
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

pub(crate) fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, error)
}

pub(crate) struct StagedImage<'file> {
	reader: RefCell<StagedReader<'file>>,
	length: u64,
}

impl<'file> StagedImage<'file> {
	pub(crate) fn new(mut reader: StagedReader<'file>) -> io::Result<Self> {
		let length = reader.seek(SeekFrom::End(0))?;
		Ok(Self {
			reader: RefCell::new(reader),
			length,
		})
	}

	#[cfg(feature = "project-release")]
	pub(crate) fn read_from_start<T>(
		&self,
		read: impl FnOnce(&mut StagedReader<'file>) -> io::Result<T>,
	) -> io::Result<T> {
		let mut reader = self.reader.try_borrow_mut().map_err(io::Error::other)?;
		reader.rewind()?;
		read(&mut reader)
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
