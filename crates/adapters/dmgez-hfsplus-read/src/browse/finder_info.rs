use std::ops::Range;

use dmgez_vfs::volume::FINDER_INFO_BYTES;
use hfsplus_format::symlink::FINDER_TYPE_AND_CREATOR;

use super::{CatalogId, Error, FinderInfo, HfsBackend, LogicalDisk, NodeKind, node_kind};

const FILE_TYPE_AND_CREATOR: Range<usize> = 0..FINDER_TYPE_AND_CREATOR.len();
const DOCUMENT_ID_AND_DATE_ADDED: Range<usize> = 16..24;
const WRITE_GENERATION: Range<usize> = 28..FINDER_INFO_BYTES;
const EMPTY_FINDER_INFO: [u8; FINDER_INFO_BYTES] = [0; FINDER_INFO_BYTES];

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn effective_finder_info(&self, node: CatalogId) -> Result<FinderInfo, Error> {
		let entry = self
			.catalog
			.entry(node)?
			.ok_or(Error::MissingFinderInfoOwner)?;
		let kind = node_kind(&entry);
		let info = if kind == NodeKind::RegularFile {
			self.regular_file(node)?.metadata().finder_info
		} else {
			entry.metadata.finder_info
		};
		Ok(macos_finder_info(kind, info))
	}
}

fn macos_finder_info(kind: NodeKind, mut info: [u8; FINDER_INFO_BYTES]) -> FinderInfo {
	// Match the visible HFS+ FinderInfo attribute, which excludes private catalog fields.
	if matches!(
		kind,
		NodeKind::RegularFile | NodeKind::Symlink | NodeKind::Directory
	) {
		info[DOCUMENT_ID_AND_DATE_ADDED].fill(0);
		info[WRITE_GENERATION].fill(0);
	}
	if kind == NodeKind::Symlink {
		info[FILE_TYPE_AND_CREATOR].fill(0);
	}
	if info == EMPTY_FINDER_INFO {
		FinderInfo::Absent
	} else {
		FinderInfo::Present(info)
	}
}

#[cfg(test)]
mod tests;
