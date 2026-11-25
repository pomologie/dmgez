use std::num::NonZeroU32;

use hfsplus_format::attributes::MAX_NAME_UNITS;
use hfsplus_reader::attributes::AttributeReadError;

use super::{
	CatalogId, Error, ForkReader, HfsBackend, LogicalDisk, LookupName, NodeKind, RawName, ReadFork,
	node_kind,
};

impl<S: LogicalDisk> HfsBackend<'_, '_, S> {
	pub(super) fn extended_attribute_names(
		&self,
		node: CatalogId,
		limit: NonZeroU32,
	) -> Result<Vec<RawName>, Error> {
		let inode = self.separate_inode(node)?;
		let mut limits = self.limits.logical_data.attributes;
		let requested = usize::try_from(limit.get()).unwrap_or(usize::MAX);
		limits.max_entries = limits.max_entries.min(requested);
		let attributes = self.volume.attributes(limits)?;
		let mut names = attributes.list_attrs(node)?;
		if let Some(inode) = inode {
			let inode_names = attributes.list_attrs(inode)?;
			names.try_reserve(inode_names.len())?;
			names.extend(inode_names);
			names.sort_unstable_by(|left, right| left.name.cmp(&right.name));
			names.dedup_by(|left, right| left.name == right.name);
			if names.len() > limits.max_entries {
				return Err(AttributeReadError::EntryLimit.into());
			}
		}
		Ok(names
			.into_iter()
			.map(|entry| RawName::Utf16(entry.name))
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
		let inode = self.separate_inode(node)?;
		let attributes = self
			.volume
			.attributes(self.limits.logical_data.attributes)?;
		let visible = attributes.open_attribute(node, units)?;
		let stored = inode
			.map(|owner| attributes.open_attribute(owner, units))
			.transpose()?
			.flatten();
		if visible.is_some() && stored.is_some() {
			return Err(Error::AmbiguousAttributeOwner);
		}
		Ok(visible.or(stored).map(|value| read(&mut ReadFork(value))))
	}

	fn separate_inode(&self, node: CatalogId) -> Result<Option<CatalogId>, Error> {
		let entry = self
			.catalog
			.entry(node)?
			.ok_or(Error::MissingAttributeOwner)?;
		if node_kind(&entry) != NodeKind::RegularFile {
			return Ok(None);
		}
		let inode = self.regular_file(node)?.inode_id();
		Ok((inode != node).then_some(inode))
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
