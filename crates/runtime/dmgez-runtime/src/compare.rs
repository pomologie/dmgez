//! File-based workflows for bounded byte and filesystem comparison.

use std::{fs::File, path::Path};

use dmgez_diff::{Report, Side, compare};
use dmgez_udif::{
	Error as ImageError,
	reader::{FileSource, Image, Limits},
};
use dmgez_vfs::Raw;
use thiserror::Error;

#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
pub mod semantic;

/// Byte address space in which the inputs are compared.
#[derive(Debug, Clone, Copy)]
pub enum ByteSpace {
	/// Stored file bytes, including all container metadata.
	Physical,
	/// Decoded logical disk bytes from two qualified UDIF images.
	Logical(Limits),
}

/// A read or admission failure associated with one input.
#[derive(Debug, Error)]
#[error("could not admit the {side} input")]
pub struct InputError<E> {
	pub side: Side,
	#[source]
	pub source: E,
}

impl<E> InputError<E> {
	/// Associates a cause with the input being admitted.
	#[must_use]
	pub const fn new(side: Side, source: E) -> Self {
		Self { side, source }
	}
}

/// Unsuccessful admission or incomplete byte comparison.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ByteError {
	#[error(transparent)]
	Input(#[from] InputError<ImageError>),
	#[error(transparent)]
	Compare(#[from] dmgez_diff::Error),
}

/// Opens, admits and compares two regular files in the selected address space.
///
/// # Errors
/// Returns a side-specific admission failure, a read error, cancellation, or
/// exhaustion of the comparison budget.
pub fn files(
	left: &Path,
	right: &Path,
	address_space: ByteSpace,
	max_bytes_per_input: u64,
	cancelled: impl FnMut() -> bool,
) -> Result<Report, ByteError> {
	let left = open_file(left).map_err(|source| InputError::new(Side::Left, source))?;
	let right = open_file(right).map_err(|source| InputError::new(Side::Right, source))?;
	let report = match address_space {
		ByteSpace::Physical => compare(
			&mut Raw(left),
			&mut Raw(right),
			max_bytes_per_input,
			cancelled,
		),
		ByteSpace::Logical(limits) => {
			let mut left = Image::open_release(left, limits)
				.map_err(|source| InputError::new(Side::Left, source))?;
			let mut right = Image::open_release(right, limits)
				.map_err(|source| InputError::new(Side::Right, source))?;
			compare(&mut left, &mut right, max_bytes_per_input, cancelled)
		}
	}?;
	Ok(report)
}

fn open_file(path: &Path) -> Result<FileSource, ImageError> {
	require_regular_file(path)?;
	FileSource::new(File::open(path)?)
}

fn require_regular_file(path: &Path) -> Result<(), ImageError> {
	if !path.metadata()?.is_file() {
		return Err(ImageError::InvalidFormat(
			"comparison input is not a regular file",
		));
	}
	Ok(())
}
