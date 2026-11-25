use std::{error::Error as StdError, num::NonZeroU32};

use apfs_core::ReadAt;
use apfs_reader::{Error as NativeError, InodeId, Limit};
use dmgez_vfs::volume::RawName;

use super::{AdapterError, ApfsBackend};

impl<S: ReadAt> ApfsBackend<'_, S>
where
	S::Error: StdError + 'static,
{
	pub(super) fn attribute_names(
		&mut self,
		owner: InodeId,
		limit: NonZeroU32,
	) -> Result<Vec<RawName>, AdapterError<S::Error>> {
		let stored = match self.view.list_xattr_names(owner, limit) {
			Err(NativeError::Limit(Limit::XattrNames)) => {
				return Err(AdapterError::AttributeLimit { limit });
			}
			other => other?,
		}
		.ok_or(AdapterError::MissingAttributeOwner)?;
		let mut names = Vec::new();
		names
			.try_reserve_exact(stored.len())
			.map_err(|_| AdapterError::AttributeAllocation)?;
		names.extend(stored.into_iter().map(RawName::Utf8));
		Ok(names)
	}
}
