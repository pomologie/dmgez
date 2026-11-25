use std::cell::RefCell;

use apfs_core::{ByteOffset, ReadAt};
use dmgez_vfs::{Error as DiskError, LogicalDisk, Window};
use thiserror::Error;

/// Mutable logical read cache behind APFS's immutable-source interface.
pub struct WindowSource<'disk, S> {
	length: u64,
	window: RefCell<Window<'disk, S>>,
}

impl<'disk, S: LogicalDisk> WindowSource<'disk, S> {
	#[must_use]
	pub fn new(window: Window<'disk, S>) -> Self {
		Self {
			length: window.len(),
			window: RefCell::new(window),
		}
	}
}

#[derive(Debug, Error)]
pub enum SourceError {
	#[error("APFS window already has an active positional read")]
	ActiveRead,
	#[error("APFS window read failed")]
	Disk(#[from] DiskError),
}

impl<S: LogicalDisk> ReadAt for WindowSource<'_, S> {
	type Error = SourceError;

	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		self.window
			.try_borrow_mut()
			.map_err(|_| SourceError::ActiveRead)?
			.read_exact_at(offset.0, output)?;
		Ok(())
	}
}
