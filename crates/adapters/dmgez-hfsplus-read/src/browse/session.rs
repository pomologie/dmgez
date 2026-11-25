use std::cell::{BorrowMutError, RefCell};

use dmgez_vfs::{Disk, LogicalDisk, VolumeId, Window};
use hfsplus_core::{ByteLen, ByteOffset, ReadAt};
use hfsplus_reader::{Volume, catalog::Catalog};

use super::{Error, Limits};

/// Failure reading from a budgeted HFS+ or HFSX volume window.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
	#[error("filesystem read attempted to reenter its source")]
	Reentrant(#[from] BorrowMutError),
	#[error("HFS+ volume read request budget exceeded")]
	RequestLimit,
	#[error("HFS+ volume read byte budget exceeded")]
	ByteLimit,
	#[error(transparent)]
	Window(#[from] dmgez_vfs::Error),
}

/// Positioned reads restricted to one selected, budgeted volume window.
pub struct WindowSource<'disk, S> {
	read: RefCell<BudgetedWindow<'disk, S>>,
	length: ByteLen,
}

struct BudgetedWindow<'disk, S> {
	window: Window<'disk, S>,
	requests_remaining: u64,
	bytes_remaining: u64,
}

impl<S: LogicalDisk> BudgetedWindow<'_, S> {
	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		self.requests_remaining = self
			.requests_remaining
			.checked_sub(1)
			.ok_or(SourceError::RequestLimit)?;
		self.bytes_remaining = self
			.bytes_remaining
			.checked_sub(output.len() as u64)
			.ok_or(SourceError::ByteLimit)?;
		Ok(self.window.read_exact_at(offset, output)?)
	}
}

impl<S: LogicalDisk> ReadAt for WindowSource<'_, S> {
	type Error = SourceError;

	fn len(&self) -> ByteLen {
		self.length
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), SourceError> {
		self.read.try_borrow_mut()?.read_exact_at(offset.0, output)
	}
}

/// Lends the selected native volume and validated catalog for a read session.
/// Callers may keep stored fork readers open inside the callback. Volume,
/// catalog, attribute and fork reads share the same positional I/O budgets.
/// Callers supply the native limits when opening individual readers.
/// # Errors
/// Returns selection and admission failures before invoking `read`.
pub fn with_catalog<'disk, S: LogicalDisk, T>(
	disk: &'disk mut Disk<S>,
	selected: VolumeId,
	limits: Limits,
	read: impl for<'volume> FnOnce(
		&'volume Volume<WindowSource<'disk, S>>,
		Catalog<'volume, WindowSource<'disk, S>>,
	) -> T,
) -> Result<T, Error> {
	let window = disk.data_window(selected)?;
	let source = WindowSource {
		length: ByteLen(window.len()),
		read: RefCell::new(BudgetedWindow {
			window,
			requests_remaining: limits.max_read_requests,
			bytes_remaining: limits.max_read_bytes,
		}),
	};
	let volume = Volume::open(source)?;
	let catalog = volume.open_catalog(limits.catalog)?;
	Ok(read(&volume, catalog))
}
