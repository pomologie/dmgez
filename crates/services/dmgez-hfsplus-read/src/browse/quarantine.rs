use hfsplus_reader::catalog::ROOT_ID;

use super::{
	CatalogId, Error, HfsBackend, LogicalDisk, NodeKind, Quarantine, QuarantineScope, node_kind,
};

const QUARANTINE_ATTRIBUTE: &str = "com.apple.quarantine";

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn check_quarantine(
		&self,
		node: CatalogId,
		scope: QuarantineScope,
	) -> Result<Quarantine, Error> {
		let fork = self.volume.header().forks.attributes;
		if fork.logical_bytes == 0 && fork.total_blocks == 0 {
			return Ok(Quarantine::Clear);
		}
		// The catalog verifies the visible ancestor chain, including cycle detection.
		let Some(entry) = self.catalog.entry(node)? else {
			return Ok(Quarantine::Unavailable);
		};
		let attributes = self
			.volume
			.attributes(self.limits.logical_data.attributes)?;
		let name: [u16; QUARANTINE_ATTRIBUTE.len()] =
			std::array::from_fn(|index| u16::from(QUARANTINE_ATTRIBUTE.as_bytes()[index]));
		// Opening a fork-backed attribute validates its metadata without reading its payload.
		let marked = |owner| Ok::<_, Error>(attributes.open_attribute(owner, &name)?.is_some());
		if marked(node)? {
			return Ok(Quarantine::Marked);
		}
		if node_kind(&entry) == NodeKind::RegularFile {
			let inode = self.regular_file(node)?.inode_id();
			if inode != node && marked(inode)? {
				return Ok(Quarantine::Marked);
			}
		}
		if scope == QuarantineScope::Object || node == ROOT_ID {
			return Ok(Quarantine::Clear);
		}
		let mut parent = entry.parent;
		for _ in 0..self.limits.catalog.max_ancestors {
			if marked(parent)? {
				return Ok(Quarantine::Marked);
			}
			if parent == ROOT_ID {
				return Ok(Quarantine::Clear);
			}
			let Some(ancestor) = self.catalog.entry(parent)? else {
				return Ok(Quarantine::Unavailable);
			};
			parent = ancestor.parent;
		}
		Ok(Quarantine::Unavailable)
	}
}
