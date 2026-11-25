use std::error::Error as StdError;

use apfs_core::ReadAt;
use apfs_reader::{FileKind, InodeId};
use dmgez_vfs::volume::{Quarantine, QuarantineScope};

use super::{AdapterError, ApfsBackend, Node};

const QUARANTINE_ATTRIBUTE: &str = "com.apple.quarantine";
const MAX_QUARANTINE_ANCESTORS: usize = 128;

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
		let Some(mut parent) = node.parent else {
			return Ok(Quarantine::Unavailable);
		};
		if node.sibling.is_none() && inode.parent != Some(parent) {
			return Ok(Quarantine::Unavailable);
		}
		let mut visited = [InodeId::ROOT; MAX_QUARANTINE_ANCESTORS];
		for index in 0..MAX_QUARANTINE_ANCESTORS {
			if visited[..index].contains(&parent) {
				return Ok(Quarantine::Unavailable);
			}
			visited[index] = parent;
			let Some(ancestor) = self.view.stat(parent)? else {
				return Ok(Quarantine::Unavailable);
			};
			if ancestor.kind != FileKind::Directory {
				return Ok(Quarantine::Unavailable);
			}
			if self.has_quarantine(ancestor.id)? {
				return Ok(Quarantine::Marked);
			}
			if ancestor.id == InodeId::ROOT {
				return Ok(Quarantine::Clear);
			}
			let Some(next) = ancestor.parent else {
				return Ok(Quarantine::Unavailable);
			};
			parent = next;
		}
		Ok(Quarantine::Unavailable)
	}

	fn has_quarantine(&mut self, id: InodeId) -> Result<bool, AdapterError<S::Error>> {
		Ok(self.view.open_xattr(id, QUARANTINE_ATTRIBUTE)?.is_some())
	}
}
