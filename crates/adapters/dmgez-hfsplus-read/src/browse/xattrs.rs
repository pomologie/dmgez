use std::num::NonZeroU32;

use hfsplus_format::attributes::MAX_NAME_UNITS;
use hfsplus_reader::{attributes::AttributeReadError, catalog::CatalogAttributeError};

use super::{
	CatalogId, Error, ForkReader, HfsBackend, LogicalDisk, LookupName, RawName, ReadFork, SourceError,
};

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn extended_attribute_names(
		&self,
		node: CatalogId,
		limit: NonZeroU32,
	) -> Result<Vec<RawName>, Error> {
		let mut limits = self.limits.logical_data.attributes;
		let requested = usize::try_from(limit.get()).unwrap_or(usize::MAX);
		limits.max_entries = limits.max_entries.min(requested);
		let names = self
			.catalog
			.list_entry_attribute_names(node, self.limits.hard_links, limits)?;
		Ok(names
			.into_iter()
			.map(RawName::Utf16)
			.collect())
	}

	pub(super) fn with_extended_attribute<T>(
		&self,
		node: CatalogId,
		name: LookupName<'_>,
		read: impl FnOnce(&mut dyn ForkReader<Error = Error>) -> T,
	) -> Result<Option<T>, Error> {
		let mut buffer = [0; MAX_NAME_UNITS];
		let units = match name {
			LookupName::Utf8(name) => encode_attribute_name(name, &mut buffer)?,
			LookupName::StoredUtf16(units) => units,
			_ => return Err(Error::LookupEncoding),
		};
		let attribute = self.catalog.open_entry_attribute(
			node,
			units,
			self.limits.hard_links,
			self.limits.logical_data.attributes,
		)?;
		Ok(attribute.map(|value| read(&mut ReadFork(value))))
	}
}

	impl From<CatalogAttributeError<SourceError>> for Error {
	fn from(error: CatalogAttributeError<SourceError>) -> Self {
		match error {
			CatalogAttributeError::Catalog(error) => Self::Catalog(error),
			CatalogAttributeError::HardLink(error) => Self::HardLink(error),
			CatalogAttributeError::Attributes(error) => Self::Attributes(error),
			CatalogAttributeError::MissingOwner(_) => Self::MissingAttributeOwner,
			CatalogAttributeError::MissingRegularFile(_) => Self::MissingRegularFile,
			CatalogAttributeError::AmbiguousOwner { .. } => Self::AmbiguousAttributeOwner,
			other => Self::CatalogAttributes(other),
		}
	}
}

fn encode_attribute_name<'a>(
	name: &str,
	buffer: &'a mut [u16; MAX_NAME_UNITS],
) -> Result<&'a [u16], Error> {
	let mut units = name.encode_utf16();
	let mut length = 0;
	for (slot, unit) in buffer.iter_mut().zip(units.by_ref()) {
		*slot = unit;
		length += 1;
	}
	if units.next().is_some() {
		return Err(AttributeReadError::InvalidName.into());
	}
	Ok(&buffer[..length])
}
