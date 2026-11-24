use std::ops::Range;

use crate::{Error, LogicalDisk};

/// Byte-exact view scoped to an admitted disk extent.
/// Subwindows inherit the parent's range restriction and source checks.
#[derive(Debug)]
pub struct Window<'source, S> {
	source: &'source mut S,
	range: Range<u64>,
}

impl<'source, S: LogicalDisk> Window<'source, S> {
	pub(super) const fn new(source: &'source mut S, range: Range<u64>) -> Self {
		Self { source, range }
	}

	/// Length of this bounded extent in bytes.
	#[must_use]
	pub const fn len(&self) -> u64 {
		self.range.end - self.range.start
	}

	#[must_use]
	pub const fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// The absolute logical-disk offset of the first byte.
	#[must_use]
	pub const fn logical_offset(&self) -> u64 {
		self.range.start
	}

	/// Read exactly within this window, using a relative byte offset.
	///
	/// # Errors
	/// Fails on range violation, source change, or source read error.
	pub fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		let absolute = self.translate(offset, output.len() as u64)?;
		Ok(self.source.read_exact_at(absolute.start, output)?)
	}

	/// Reborrow a bounded subrange of this window.
	///
	/// # Errors
	/// Fails if the complete subwindow does not fit inside the parent window.
	pub fn subwindow(&mut self, offset: u64, len: u64) -> Result<Window<'_, S>, Error> {
		let absolute = self.translate(offset, len)?;
		Ok(Window::new(self.source, absolute))
	}

	fn translate(&self, offset: u64, len: u64) -> Result<Range<u64>, Error> {
		translate(&self.range, offset, len).ok_or(Error::OutOfRange {
			offset,
			len,
			available: self.len(),
		})
	}
}

pub(super) fn translate(parent: &Range<u64>, offset: u64, len: u64) -> Option<Range<u64>> {
	let start = parent.start.checked_add(offset)?;
	let end = start.checked_add(len)?;
	(end <= parent.end).then_some(start..end)
}
