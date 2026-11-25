use std::num::NonZeroU16;

use dmgez_vfs::{LogicalDisk, volume::FileDataRanges};
use hfsplus_core::ReadAt;
use hfsplus_format::{bsd, volume::CatalogId};
use hfsplus_reader::overflow::{ForkKind, OverflowError};

use super::{Error, HfsBackend};

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn direct_data_ranges(
		&self,
		node: CatalogId,
		limit: NonZeroU16,
	) -> Result<FileDataRanges, Error> {
		let file = self.regular_file(node)?;
		if file.metadata().permissions.owner_flags & bsd::COMPRESSED != 0 {
			return Ok(FileDataRanges::Unavailable);
		}
		let fork = match file.open_stored_fork(ForkKind::Data, self.limits.stored_fork) {
			Ok(fork) => fork,
			Err(
				OverflowError::BootstrapOverflow
				| OverflowError::Read(hfsplus_reader::Error::UnsupportedVolume(_)),
			) => {
				return Ok(FileDataRanges::Unavailable);
			}
			Err(OverflowError::Limit(_) | OverflowError::Allocation(_)) => {
				return Ok(FileDataRanges::Limit);
			}
			Err(error) => return Err(error.into()),
		};
		let ranges = fork.extents().scan(fork.len().0, |remaining, extent| {
			if *remaining == 0 {
				return None;
			}
			Some(
				self.geometry
					.block_range(extent.start, extent.blocks)
					.map(|extent| {
						let count = (extent.end - extent.start).min(*remaining);
						*remaining -= count;
						extent.start..extent.start + count
					})
					.map_err(|error| Error::Volume(hfsplus_reader::Error::Format(error))),
			)
		});
		FileDataRanges::collect(ranges, limit)
	}
}
