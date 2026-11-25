use std::{num::NonZeroU16, ops::Range};

use super::{NodeHandle, NodeKind, ViewError, VolumeBackend, VolumeView};

/// Backing ranges of one regular file's logical data in an admitted volume or container.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileDataRanges {
	/// The volume-relative ranges. An empty vector establishes that no disk bytes back the stream.
	Mapped(Vec<Range<u64>>),
	/// The storage profile does not establish a direct mapping for this stream.
	Unavailable,
	/// An observation budget prevented collecting the complete set of ranges.
	Limit,
}

impl FileDataRanges {
	/// Collects checked volume-relative ranges within a caller's extent budget.
	///
	/// # Errors
	/// Propagates a native range admission error occurring within the budget.
	pub fn collect<E>(
		ranges: impl IntoIterator<Item = Result<Range<u64>, E>>,
		limit: NonZeroU16,
	) -> Result<Self, E> {
		let mut mapped = Vec::new();
		for range in ranges {
			if mapped.len() == usize::from(limit.get()) || mapped.try_reserve(1).is_err() {
				return Ok(Self::Limit);
			}
			mapped.push(range?);
		}
		Ok(Self::Mapped(mapped))
	}
}

impl<B: VolumeBackend> VolumeView<B> {
	/// Returns evidence of volume-relative data ranges for an exact visible regular file.
	///
	/// # Errors
	/// Rejects foreign and non-file handles and propagates backend admission failures.
	pub fn file_data_ranges(
		&mut self,
		node: &NodeHandle<B::Id>,
		limit: NonZeroU16,
	) -> Result<FileDataRanges, ViewError<B::Error>> {
		let local = self.require_kind(node, NodeKind::RegularFile)?;
		let ranges = self
			.backend
			.file_data_ranges(local, limit)
			.map_err(ViewError::Backend)?;
		if matches!(&ranges, FileDataRanges::Mapped(ranges) if ranges.len() > usize::from(limit.get()))
		{
			return Ok(FileDataRanges::Limit);
		}
		Ok(ranges)
	}
}
