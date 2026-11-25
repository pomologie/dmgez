//! Encode Finder item records using the selected filesystem's stored names.

use std::{borrow::Cow, error::Error as StdError};

type EncodeName = fn(&str) -> Result<String, Box<dyn StdError + Send + Sync>>;

/// A spelling policy supplied by the filesystem adapter.
///
/// The default preserves the input spelling. A filesystem that changes names
/// on disk can supply a conversion from manifest spelling to stored spelling.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogNames(Option<EncodeName>);

impl CatalogNames {
	/// Use the supplied filesystem spelling for named Finder records.
	#[must_use]
	pub const fn new(encode: EncodeName) -> Self {
		Self(Some(encode))
	}

	pub(super) fn stored(self, name: &str) -> Result<Cow<'_, str>, crate::Error> {
		match self.0 {
			Some(encode) => encode(name)
				.map(Cow::Owned)
				.map_err(crate::Error::CatalogName),
			None => Ok(Cow::Borrowed(name)),
		}
	}
}
