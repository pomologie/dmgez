use std::{error::Error as StdError, num::NonZeroU16};

use apfs_core::ReadAt;
use apfs_reader::InodeId;
use dmgez_vfs::volume::{Quarantine, QuarantineScope};

use super::{AdapterError, ApfsBackend, Node};

const QUARANTINE_ATTRIBUTE: &str = "com.apple.quarantine";
const MAX_QUARANTINE_ANCESTORS: NonZeroU16 = NonZeroU16::new(128).unwrap();

impl<S: ReadAt> ApfsBackend<'_, S>
where
	S::Error: StdError + 'static,
{
	pub(super) fn check_quarantine(
		&mut self,
		node: Node,
		scope: QuarantineScope,
	) -> Result<Quarantine, AdapterError<S::Error>> {
		// An absent inode and an absent attribute have the same open_xattr result.
		let Some(inode) = self.view.stat(node.inode)? else {
			return Ok(Quarantine::Unavailable);
		};
		if self.has_quarantine(inode.id)? {
			return Ok(Quarantine::Marked);
		}
		if scope == QuarantineScope::Object || inode.id == InodeId::ROOT {
			return Ok(Quarantine::Clear);
		}
		// Hard-link attributes belong to the shared inode. The selected sibling's
		// admitted directory record supplies its ancestry.
		let Some(parent) = node.parent else {
			return Ok(Quarantine::Unavailable);
		};
		if node.sibling.is_none() && inode.parent != Some(parent) {
			return Ok(Quarantine::Unavailable);
		}
		Ok(
			match self.view.directory_ancestry_has_xattr(
				parent,
				QUARANTINE_ATTRIBUTE,
				MAX_QUARANTINE_ANCESTORS,
			)? {
				Some(true) => Quarantine::Marked,
				Some(false) => Quarantine::Clear,
				None => Quarantine::Unavailable,
			},
		)
	}

	fn has_quarantine(&mut self, id: InodeId) -> Result<bool, AdapterError<S::Error>> {
		Ok(self.view.open_xattr(id, QUARANTINE_ATTRIBUTE)?.is_some())
	}
}
