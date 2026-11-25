//! Encode Finder item records using the selected filesystem's stored names.

use std::{borrow::Cow, error::Error as StdError};

type EncodeName = fn(&str) -> Result<String, Box<dyn StdError + Send + Sync>>;
type CompareName = fn(&str) -> Result<Vec<u16>, Box<dyn StdError + Send + Sync>>;

/// A spelling and comparison policy supplied by the filesystem adapter.
///
/// The default preserves the input spelling. A filesystem that changes names
/// on disk can supply a conversion from manifest spelling to stored spelling.
/// The default comparison folds ASCII case.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogNames {
	encode: Option<EncodeName>,
	compare: Option<CompareName>,
}

impl CatalogNames {
	/// Use the supplied filesystem spelling for named Finder records.
	#[must_use]
	pub const fn new(encode: EncodeName) -> Self {
		Self {
			encode: Some(encode),
			compare: None,
		}
	}

	/// Use the filesystem's equality rules when selecting named destinations.
	#[must_use]
	pub const fn with_comparison_key(mut self, compare: CompareName) -> Self {
		self.compare = Some(compare);
		self
	}

	/// Returns an equality key for one catalog name.
	///
	/// # Errors
	/// Returns an error if the filesystem cannot represent or compare the name.
	pub fn comparison_key(self, name: &str) -> Result<Vec<u16>, crate::Error> {
		match self.compare {
			Some(compare) => compare(name).map_err(crate::Error::CatalogName),
			None => Ok(name.to_ascii_lowercase().encode_utf16().collect()),
		}
	}

	pub(super) fn stored(self, name: &str) -> Result<Cow<'_, str>, crate::Error> {
		match self.encode {
			Some(encode) => encode(name)
				.map(Cow::Owned)
				.map_err(crate::Error::CatalogName),
			None => Ok(Cow::Borrowed(name)),
		}
	}
}
