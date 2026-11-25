use std::error::Error as StdError;

use crate::window::translate;

use super::ViewError;

/// Engine operator for exact positional reads of one admitted fork.
pub trait ForkReader {
	type Error: StdError + 'static;

	fn len(&self) -> u64;

	fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Fills the buffer exactly from the selected fork, with no implicit fallback.
	///
	/// # Errors
	/// Reports range, encoding, integrity and source failures.
	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Self::Error>;
}

/// Bounded exact read of one file's selected fork.
pub struct Fork<'reader, E> {
	reader: &'reader mut dyn ForkReader<Error = E>,
	length: u64,
}

impl<'reader, E: StdError + 'static> Fork<'reader, E> {
	pub(super) fn new(reader: &'reader mut dyn ForkReader<Error = E>) -> Self {
		let length = reader.len();
		Self { reader, length }
	}

	#[must_use]
	pub const fn len(&self) -> u64 {
		self.length
	}

	#[must_use]
	pub const fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Reads exactly the requested bytes, including an empty read at EOF.
	///
	/// # Errors
	/// Rejects out-of-bounds or overflowing ranges and propagates read failures.
	pub fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), ViewError<E>> {
		let len = output.len() as u64;
		if translate(&(0..self.length), offset, len).is_none() {
			return Err(ViewError::OutOfRange {
				offset,
				len,
				available: self.length,
			});
		}
		self.reader
			.read_exact_at(offset, output)
			.map_err(ViewError::Backend)
	}
}
