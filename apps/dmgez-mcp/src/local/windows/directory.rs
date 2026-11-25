use std::{
	ffi::{OsStr, OsString},
	fs::File,
	io,
	mem::offset_of,
	os::windows::ffi::OsStringExt as _,
};

use windows_namespace_request_sys::{AlignedBuf, FileInformationClass, QueryFileInformation};
use windows_sys::Win32::{
	Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES},
	Storage::FileSystem::{FILE_ID_128, FILE_ID_EXTD_DIR_INFO},
};

use super::MAX_NAME_UTF16;

const DIRECTORY_BUFFER_BYTES: usize = 4096;
const RECORD_ALIGNMENT: usize = 8;
const NAME_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
const NAME_LENGTH_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileNameLength);
const NEXT_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, NextEntryOffset);
const ID_OFFSET: usize =
	offset_of!(FILE_ID_EXTD_DIR_INFO, FileId) + offset_of!(FILE_ID_128, Identifier);
const ATTRIBUTES_OFFSET: usize = offset_of!(FILE_ID_EXTD_DIR_INFO, FileAttributes);
const ID_BYTES: usize = 16;

pub(in super::super) struct Directory {
	file: File,
	batch: Option<Batch>,
	started: bool,
}

struct Batch {
	bytes: AlignedBuf,
	offset: usize,
}

pub(in super::super) struct Entry {
	name: OsString,
	pub(super) id: [u8; ID_BYTES],
	pub(super) attributes: u32,
}

impl Entry {
	pub(in super::super) fn name(&self) -> &OsStr {
		&self.name
	}
}

impl Directory {
	pub(super) const fn new(file: File) -> Self {
		Self {
			file,
			batch: None,
			started: false,
		}
	}

	pub(in super::super) fn next_entry(&mut self) -> io::Result<Option<Entry>> {
		if self.batch.is_none() {
			self.batch = self.query()?;
		}
		let Some(batch) = self.batch.as_mut() else {
			return Ok(None);
		};
		let (entry, next) = parse_entry(batch.bytes.as_ref(), batch.offset)?;
		if let Some(next) = next {
			batch.offset = next;
		} else {
			self.batch = None;
		}
		Ok(Some(entry))
	}

	fn query(&mut self) -> io::Result<Option<Batch>> {
		let class = if self.started {
			FileInformationClass::ID_EXTD_DIRECTORY
		} else {
			FileInformationClass::ID_EXTD_DIRECTORY_RESTART
		};
		// The duplicate shares the cursor. At most one MCP operation scans at a time.
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

fn parse_entry(bytes: &[u8], offset: usize) -> io::Result<(Entry, Option<usize>)> {
	let record = bytes.get(offset..).ok_or_else(malformed)?;
	let next_delta = read_u32(record, NEXT_OFFSET)? as usize;
	let name_bytes = read_u32(record, NAME_LENGTH_OFFSET)? as usize;
	let units = name_bytes / size_of::<u16>();
	if name_bytes == 0 || !name_bytes.is_multiple_of(size_of::<u16>()) || units > MAX_NAME_UTF16 {
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
	let mut wide = [0_u16; MAX_NAME_UTF16];
	let (pairs, _) = name.as_chunks::<{ size_of::<u16>() }>();
	for (unit, &pair) in wide.iter_mut().zip(pairs) {
		*unit = u16::from_le_bytes(pair);
	}
	let entry = Entry {
		name: OsString::from_wide(&wide[..units]),
		id: read_bytes(record, ID_OFFSET)?,
		attributes: read_u32(record, ATTRIBUTES_OFFSET)?,
	};
	Ok((entry, next))
}

fn read_bytes<const N: usize>(record: &[u8], offset: usize) -> io::Result<[u8; N]> {
	record
		.get(offset..offset + N)
		.and_then(|bytes| bytes.try_into().ok())
		.ok_or_else(malformed)
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
	Ok(u32::from_le_bytes(read_bytes(bytes, offset)?))
}

fn malformed() -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidData,
		"invalid NTFS directory enumeration record",
	)
}

#[cfg(test)]
mod tests;
