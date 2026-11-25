use std::error::Error;

use dmgez_finder::{edit::EditableStore, inspection::ReadLimits};
use hfsplus_core::{ByteOffset, ReadAt};
use hfsplus_format::volume::CatalogId;
use hfsplus_reader::overflow::{OverflowError, ResolvedFork};

use super::{CaptureError, charge};

pub(super) fn read_store<S: ReadAt>(
	id: CatalogId,
	fork: &ResolvedFork<'_, S>,
	remaining: &mut usize,
) -> Result<EditableStore, CaptureError<S::Error>>
where
	S::Error: Error + 'static,
{
	let limits = ReadLimits::default();
	let len = usize::try_from(fork.len().0)
		.ok()
		.filter(|&len| len <= limits.store_bytes)
		.ok_or(CaptureError::FinderStoreBudget(id))?;
	if len > *remaining {
		return Err(CaptureError::Budget);
	}
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(len)?;
	bytes.resize(len, 0);
	fork.read_exact_at(ByteOffset(0), &mut bytes)
		.map_err(OverflowError::Read)?;
	let layout = EditableStore::open(&bytes, limits)
		.map_err(|error| CaptureError::FinderLayout { id, error })?;
	// Bound cumulative fork reads as well as the layout data retained for edits.
	charge(remaining, len.max(layout.retained_bytes()))?;
	Ok(layout)
}
