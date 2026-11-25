use std::{
	error::Error as StdError,
	num::{NonZeroU16, NonZeroU32},
};

use apfs_core::ReadAt;
use apfs_reader::{Error as NativeError, InodeId, Limit};
use dmgez_vfs::volume::RawName;

use super::{AdapterError, ApfsBackend};

const ATTRIBUTE_PAGE_NAMES: NonZeroU16 = NonZeroU16::new(64).unwrap();

impl<S: ReadAt> ApfsBackend<'_, S>
where
	S::Error: StdError + 'static,
{
	pub(super) fn attribute_names(
		&mut self,
		owner: InodeId,
		limit: NonZeroU32,
	) -> Result<Vec<RawName>, AdapterError<S::Error>> {
		let mut names = Vec::new();
		let mut cursor = None;
		let mut page_limit = ATTRIBUTE_PAGE_NAMES;
		loop {
			let remaining = limit.get() - names.len() as u32;
			let requested = remaining.min(u32::from(page_limit.get()));
			let page_size =
				NonZeroU16::new(requested as u16).ok_or(AdapterError::AttributeLimit { limit })?;
			let page = match self.view.list_xattrs(owner, cursor, page_size) {
				Err(NativeError::Limit(Limit::Page)) if page_size > NonZeroU16::MIN => {
					// A caller-provided view can have a narrower native page limit.
					page_limit = NonZeroU16::new(page_size.get() / 2).unwrap();
					continue;
				}
				result => result?,
			}
			.ok_or(AdapterError::MissingAttributeOwner)?;
			names
				.try_reserve(page.entries.len())
				.map_err(|_| AdapterError::AttributeAllocation)?;
			names.extend(
				page.entries
					.into_iter()
					.map(|entry| RawName::Utf8(entry.name)),
			);
			let Some(next) = page.next else {
				return Ok(names);
			};
			cursor = Some(next);
		}
	}
}
