use hfsplus_reader::catalog::FinderInfoError;

use super::{CatalogId, Error, FinderInfo, HfsBackend, LogicalDisk, SourceError};

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn effective_finder_info(&self, node: CatalogId) -> Result<FinderInfo, Error> {
		let info = self
			.catalog
			.macos_finder_info(node, self.limits.hard_links)?;
		Ok(info.map_or(FinderInfo::Absent, FinderInfo::Present))
	}
}

impl From<FinderInfoError<SourceError>> for Error {
	fn from(error: FinderInfoError<SourceError>) -> Self {
		match error {
			FinderInfoError::Catalog(error) => Self::Catalog(error),
			FinderInfoError::MissingEntry(_) => Self::MissingFinderInfoOwner,
			FinderInfoError::HardLink(error) => Self::HardLink(error),
			FinderInfoError::MissingRegularFile(_) => Self::MissingRegularFile,
			other => Self::FinderInfo(other),
		}
	}
}
