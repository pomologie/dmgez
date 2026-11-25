//! Finder folder comparisons using the selected catalog's name policy.

use dmgez_finder::{CatalogNames, Error};

use super::PATH_SEPARATOR;

pub(crate) type FolderKey = Vec<Vec<u16>>;

pub(crate) fn folder_key(path: &str, names: CatalogNames) -> Result<FolderKey, Error> {
	path.split(PATH_SEPARATOR)
		.filter(|part| !part.is_empty())
		.map(|part| names.comparison_key(part))
		.collect()
}
