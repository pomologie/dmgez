use std::{ffi::OsString, fs::File, io, mem::offset_of, os::windows::ffi::OsStringExt as _};

use windows_namespace_request_sys::{AlignedBuf, FileInformationClass, QueryFileInformation};
use windows_sys::Win32::{
	Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES},
	Storage::FileSystem::FILE_ID_EXTD_DIR_INFO,
};

pub(in super::super) const DIRECTORY_BUFFER_BYTES: usize = 4096;
const RECORD_ALIGNMENT: usize = 8;
const MAX_NAME_UNITS: usize = 255;
const NAME_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
const NAME_LENGTH_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength);
const NEXT_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset);
const NTFS_SELF: &str = ".";
const NTFS_PARENT: &str = "..";

pub(in super::super) struct Directory {
	file: File,
	batch: Option<Batch>,
	started: bool,
	finished: bool,
}

struct Batch {
	bytes: AlignedBuf,
	offset: usize,
}

impl Directory {
	pub(super) const fn new(file: File) -> Self {
		Self {
			file,
			batch: None,
			started: false,
			finished: false,
		}
	}

	pub(super) const fn file(&self) -> &File {
		&self.file
	}

	pub(super) fn next_name(&mut self) -> io::Result<Option<OsString>> {
		while !self.finished {
			if self.batch.is_none() {
				self.batch = self.query()?;
				if self.batch.is_none() {
					self.finished = true;
					break;
				}
			}
			let batch = self.batch.as_mut().ok_or_else(malformed)?;
			let (name, next) = parse_entry(batch.bytes.as_ref(), batch.offset)?;
			if let Some(next) = next {
				batch.offset = next;
			} else {
				self.batch = None;
			}
			if name != NTFS_SELF && name != NTFS_PARENT {
				return Ok(Some(name));
			}
		}
		Ok(None)
	}

	fn query(&mut self) -> io::Result<Option<Batch>> {
		let class = if self.started {
			FileInformationClass::ID_EXTD_DIRECTORY
		} else {
			FileInformationClass::ID_EXTD_DIRECTORY_RESTART
		};
		// The duplicate refers to the same open directory and enumeration cursor.
		let query = QueryFileInformation::new(self.file.try_clone()?, class)
			.with_capacity(DIRECTORY_BUFFER_BYTES);
		match query.perform() {
			Ok(bytes) => {
				self.started = true;
				Ok(Some(Batch { bytes, offset: 0 }))
			}
			Err(error)
				if error.code() == ERROR_NO_MORE_FILES
					|| (!self.started && error.code() == ERROR_FILE_NOT_FOUND) =>
			{
				Ok(None)
			}
			Err(error) => Err(error.into()),
		}
	}
}

fn parse_entry(bytes: &[u8], offset: usize) -> io::Result<(OsString, Option<usize>)> {
	let record = bytes.get(offset..).ok_or_else(malformed)?;
	let next_delta = read_u32(record, NEXT_OFFSET)?;
	let name_bytes = read_u32(record, NAME_LENGTH_OFFSET)?;
	let units = name_bytes / size_of::<u16>();
	if name_bytes == 0 || !name_bytes.is_multiple_of(size_of::<u16>()) || units > MAX_NAME_UNITS {
		return Err(malformed());
	}
	let name_end = NAME_OFFSET.checked_add(name_bytes).ok_or_else(malformed)?;
	let name = record.get(NAME_OFFSET..name_end).ok_or_else(malformed)?;
	let next = if next_delta == 0 {
		None
	} else {
		if !next_delta.is_multiple_of(RECORD_ALIGNMENT)
			|| next_delta < name_end
			|| record.len().saturating_sub(next_delta) < NAME_OFFSET
		{
			return Err(malformed());
		}
		Some(offset.checked_add(next_delta).ok_or_else(malformed)?)
	};
	let mut wide = [0_u16; MAX_NAME_UNITS];
	let (pairs, _) = name.as_chunks::<{ size_of::<u16>() }>();
	for (unit, &pair) in wide.iter_mut().zip(pairs) {
		*unit = u16::from_le_bytes(pair);
	}
	Ok((OsString::from_wide(&wide[..units]), next))
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<usize> {
	let end = offset.checked_add(size_of::<u32>()).ok_or_else(malformed)?;
	let raw = bytes.get(offset..end).ok_or_else(malformed)?;
	let raw = raw.try_into().map_err(|_| malformed())?;
	usize::try_from(u32::from_le_bytes(raw)).map_err(|_| malformed())
}

fn malformed() -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidData,
		"invalid NTFS directory enumeration record",
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	const TEST_NAME: u16 = b'x' as u16;
	const RECORD_LENGTH: usize =
		(NAME_OFFSET + size_of::<u16>()).next_multiple_of(RECORD_ALIGNMENT);

	fn record(bytes: &mut [u8], next_delta: u32, name_length: u32) {
		bytes[NEXT_OFFSET..NEXT_OFFSET + size_of::<u32>()]
			.copy_from_slice(&next_delta.to_le_bytes());
		bytes[NAME_LENGTH_OFFSET..NAME_LENGTH_OFFSET + size_of::<u32>()]
			.copy_from_slice(&name_length.to_le_bytes());
		bytes[NAME_OFFSET..NAME_OFFSET + size_of::<u16>()]
			.copy_from_slice(&TEST_NAME.to_le_bytes());
	}

	#[test]
	fn validates_the_directory_batch_record_boundaries() {
		let unit_bytes = size_of::<u16>() as u32;
		let mut bytes = [0; RECORD_LENGTH * 2];
		record(&mut bytes, RECORD_LENGTH as u32, unit_bytes);
		record(&mut bytes[RECORD_LENGTH..], 0, unit_bytes);
		let (name, next) = parse_entry(&bytes, 0).unwrap();
		assert_eq!(name, "x");
		assert_eq!(next, Some(RECORD_LENGTH));
		assert_eq!(parse_entry(&bytes, RECORD_LENGTH).unwrap().1, None);
		for (next, length) in [
			(RECORD_LENGTH as u32 - 1, unit_bytes),
			(RECORD_LENGTH as u32 * 2, unit_bytes),
			(0, unit_bytes + 1),
			(0, u32::MAX - 1),
		] {
			record(&mut bytes, next, length);
			assert_eq!(
				parse_entry(&bytes, 0).unwrap_err().kind(),
				io::ErrorKind::InvalidData
			);
		}
	}
}
