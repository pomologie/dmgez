//! Finder metadata and a single-resource fork for a custom item icon.

use std::{collections::TryReserveError, mem::size_of};

use thiserror::Error;

use crate::{
	finder_info::{self, FINDER_INFO_BYTES, HAS_CUSTOM_ICON, INVISIBLE},
	volume_icon,
};

/// The standard hidden child that carries a folder's icon resource fork.
pub const ICON_FILE_NAME: &str = "Icon\r";

// The resource fork follows Apple's classic Resource Manager format. In
// particular, NSWorkspace-written custom icon forks begin data at byte 256.
const ICON_RESOURCE_TYPE: [u8; 4] = *b"icns";
const ICON_RESOURCE_ID: i16 = -16_455;
const DATA_OFFSET: u32 = 256;
const UNNAMED_RESOURCE: i16 = -1;
const RESOURCE_COUNT_MINUS_ONE: u16 = 0;
const RESOURCE_OFFSET_AND_FLAGS: u32 = 0;
const RESERVED_RESOURCE_HANDLE: u32 = 0;
const RESERVED_NEXT_MAP_HANDLE: u32 = 0;
const RESERVED_FILE_REFERENCE: u16 = 0;
const MAP_FLAGS: u16 = 0;
const TYPE_LIST_OFFSET: u16 = 28;
// The count word and one type entry precede the list of resource references.
const REFERENCE_LIST_OFFSET: u16 = 10;
const REFERENCE_BYTES: u16 = 12;
const MAP_BYTES: u16 = TYPE_LIST_OFFSET + REFERENCE_LIST_OFFSET + REFERENCE_BYTES;
const NAME_LIST_OFFSET: u16 = MAP_BYTES;

/// The exact encoded fork size in addition to its ICNS payload.
pub const RESOURCE_FORK_OVERHEAD_BYTES: usize =
	DATA_OFFSET as usize + size_of::<u32>() + MAP_BYTES as usize;

/// An icon resource fork could not be produced within the supported profile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	/// The ICNS is outside the supported input profile.
	#[error("invalid per-item ICNS asset")]
	Icon(#[from] volume_icon::Error),
	/// The fork would exceed the available metadata budget.
	#[error("custom icon resource fork exceeds the output byte budget")]
	OutputBudget,
	/// An encoded length cannot be represented in the resource map.
	#[error("custom icon resource fork size cannot be represented")]
	Size,
	/// The encoded allocation was unsuccessful.
	#[error("cannot reserve custom icon resource fork memory")]
	Reserve(#[from] TryReserveError),
}

/// Encodes an exact ICNS payload in one unnamed custom-icon resource.
///
/// The caller accounts for the retained ICNS input and the validation workspace
/// from [`volume_icon::validation_memory`] separately. The output has a fixed
/// overhead and its allocation is checked against `max_output_bytes`.
///
/// # Errors
///
/// Returns an error for unsupported or malformed ICNS, insufficient output
/// capacity, an allocation failure, or an unrepresentable length.
pub fn encode_resource_fork(icns: &[u8], max_output_bytes: usize) -> Result<Vec<u8>, Error> {
	if icns.len() > volume_icon::MAX_BYTES {
		return Err(volume_icon::Error::Size.into());
	}
	let fork_len = icns
		.len()
		.checked_add(RESOURCE_FORK_OVERHEAD_BYTES)
		.ok_or(Error::Size)?;
	if fork_len > max_output_bytes {
		return Err(Error::OutputBudget);
	}
	let icns = volume_icon::validate(icns)?.as_bytes();
	let icon_len = u32::try_from(icns.len()).map_err(|_| Error::Size)?;
	let data_len = icon_len
		.checked_add(size_of::<u32>() as u32)
		.ok_or(Error::Size)?;
	let map_offset = DATA_OFFSET.checked_add(data_len).ok_or(Error::Size)?;
	let header = [DATA_OFFSET, map_offset, data_len, u32::from(MAP_BYTES)].map(u32::to_be_bytes);

	let mut fork = Vec::new();
	fork.try_reserve_exact(fork_len)?;
	if fork.capacity() > max_output_bytes {
		return Err(Error::OutputBudget);
	}
	fork.extend_from_slice(header.as_flattened());
	fork.resize(DATA_OFFSET as usize, 0);
	fork.extend_from_slice(&icon_len.to_be_bytes());
	fork.extend_from_slice(icns);

	fork.extend_from_slice(header.as_flattened());
	fork.extend_from_slice(&RESERVED_NEXT_MAP_HANDLE.to_be_bytes());
	fork.extend_from_slice(&RESERVED_FILE_REFERENCE.to_be_bytes());
	fork.extend_from_slice(&MAP_FLAGS.to_be_bytes());
	fork.extend_from_slice(&TYPE_LIST_OFFSET.to_be_bytes());
	fork.extend_from_slice(&NAME_LIST_OFFSET.to_be_bytes());
	fork.extend_from_slice(&RESOURCE_COUNT_MINUS_ONE.to_be_bytes());
	fork.extend_from_slice(&ICON_RESOURCE_TYPE);
	fork.extend_from_slice(&RESOURCE_COUNT_MINUS_ONE.to_be_bytes());
	fork.extend_from_slice(&REFERENCE_LIST_OFFSET.to_be_bytes());
	fork.extend_from_slice(&ICON_RESOURCE_ID.to_be_bytes());
	fork.extend_from_slice(&UNNAMED_RESOURCE.to_be_bytes());
	fork.extend_from_slice(&RESOURCE_OFFSET_AND_FLAGS.to_be_bytes());
	fork.extend_from_slice(&RESERVED_RESOURCE_HANDLE.to_be_bytes());
	Ok(fork)
}

/// Marks a file or folder as having a custom icon while preserving its metadata.
pub fn mark_custom_icon(finder_info: &mut [u8; FINDER_INFO_BYTES]) {
	finder_info::add_flags(finder_info, HAS_CUSTOM_ICON);
}

/// Returns the Finder information for a generated hidden folder icon child.
#[must_use]
pub fn folder_icon_file_finder_info() -> [u8; FINDER_INFO_BYTES] {
	let mut info = [0; FINDER_INFO_BYTES];
	finder_info::add_flags(&mut info, INVISIBLE);
	info
}
