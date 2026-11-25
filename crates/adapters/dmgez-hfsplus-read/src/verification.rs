use std::{cell::RefCell, error::Error};

use hfsplus_core::{ByteLen, ByteOffset, ReadAt};
use hfsplus_reader::{
	Volume,
	check::{CheckLimits, ReachableDataReport, StructuralReport},
};

use crate::CheckError;

/// Checks a caller-bounded volume. Offsets passed to `read` are relative to
/// that volume. The reader must serve exact bytes and reject source changes.
///
/// The result covers reachable catalog and allocation structure, with
/// coverage details in the returned report. Full file payload and global
/// filesystem checks require independent verification.
///
/// # Errors
/// Reports unsupported profiles, malformed structures, resource limits, or
/// a source read failure.
pub fn check<E: Error + 'static>(
	len: u64,
	read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<StructuralReport, CheckError<E>> {
	let source = VolumeSource {
		len: ByteLen(len),
		read: RefCell::new(read),
	};
	let volume = Volume::open(source).map_err(CheckError::Open)?;
	volume
		.check_structure(CheckLimits::default())
		.map_err(CheckError::Structure)
}

/// Checks reachable structure and reads supported stored file and attribute forks.
///
/// `max_payload_bytes` bounds the stored payload lengths, including the stored
/// representation of compressed files. The caller's separate read budget
/// must also cover metadata and enforce the selected window.
///
/// # Errors
/// Reports unsupported profiles, malformed structures, resource limits, or
/// a source read failure.
pub fn check_reachable_data<E: Error + 'static>(
	len: u64,
	max_payload_bytes: u64,
	read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<ReachableDataReport, CheckError<E>> {
	let source = VolumeSource {
		len: ByteLen(len),
		read: RefCell::new(read),
	};
	let volume = Volume::open(source).map_err(CheckError::Open)?;
	volume
		.check_reachable_data(CheckLimits::default(), ByteLen(max_payload_bytes))
		.map_err(CheckError::ReachableData)
}

struct VolumeSource<F> {
	len: ByteLen,
	read: RefCell<F>,
}

impl<F, E> ReadAt for VolumeSource<F>
where
	F: FnMut(u64, &mut [u8]) -> Result<(), E>,
{
	type Error = E;

	fn len(&self) -> ByteLen {
		self.len
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), E> {
		(self.read.borrow_mut())(offset.0, output)
	}
}
