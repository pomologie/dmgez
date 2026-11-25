use std::{collections::BTreeMap, error::Error};

use hfsplus_builder::volume::validate_inline_attribute;
use hfsplus_core::ReadAt;
use hfsplus_format::{
	catalog::{FLAG_HAS_ATTRIBUTES, Metadata},
	volume::CatalogId,
};
use hfsplus_reader::{Volume, attributes::AttributeReadLimits};

use super::{CaptureError, MAX_UTF8_BYTES_PER_UTF16_UNIT, charge};
use crate::rebuild::Attribute;

const MAX_ATTRIBUTE_NODES: usize = 8 << 10;
const MAX_INLINE_ATTRIBUTES: usize = 64 << 10;

pub(super) struct AttributeInventory(BTreeMap<CatalogId, Vec<Attribute>>);

impl AttributeInventory {
	pub(super) fn read<S: ReadAt>(
		volume: &Volume<S>,
		remaining: &mut usize,
	) -> Result<Self, CaptureError<S::Error>>
	where
		S::Error: Error + 'static,
	{
		let attributes = volume.attributes(AttributeReadLimits {
			max_nodes: MAX_ATTRIBUTE_NODES,
			max_entries: MAX_INLINE_ATTRIBUTES,
		})?;
		let mut owners = BTreeMap::<CatalogId, Vec<Attribute>>::new();
		attributes.visit_inline_attrs::<CaptureError<S::Error>>(|owner, name, value| {
			let name_len = name.len() * MAX_UTF8_BYTES_PER_UTF16_UNIT;
			charge(remaining, name_len + value.len() + size_of::<Attribute>())?;
			let mut text = String::new();
			text.try_reserve_exact(name_len)?;
			for unit in char::decode_utf16(name.units()) {
				text.push(unit.map_err(|_| CaptureError::AttributeName(owner))?);
			}
			validate_inline_attribute(&text, value.len())
				.map_err(|error| CaptureError::AttributeValue { owner, error })?;
			let mut bytes = Vec::new();
			bytes.try_reserve_exact(value.len())?;
			bytes.extend_from_slice(value);
			let attributes = owners.entry(owner).or_default();
			attributes.try_reserve(1)?;
			attributes.push((text, bytes));
			Ok(())
		})?;
		Ok(Self(owners))
	}

	pub(super) fn take<E: Error + 'static>(
		&mut self,
		owner: &Metadata,
	) -> Result<Vec<Attribute>, CaptureError<E>> {
		let attributes = self.0.remove(&owner.id);
		if attributes.is_some() != (owner.flags & FLAG_HAS_ATTRIBUTES != 0) {
			return Err(CaptureError::AttributeOwner(owner.id));
		}
		Ok(attributes.unwrap_or_default())
	}

	pub(super) fn check_consumed<E: Error + 'static>(&self) -> Result<(), CaptureError<E>> {
		if let Some((&owner, _)) = self.0.first_key_value() {
			return Err(CaptureError::AttributeOwner(owner));
		}
		Ok(())
	}
}
