use std::io::Cursor;

use ds_parser::layout::BTreeMaster;
use plist::Value;

use crate::{
	BackgroundAliasError, ImageBackgroundIdentity, build_image_background_alias,
	inspection::{BackgroundAliasName, BackgroundAliasTarget, HfsCreationTime, ReadError},
	layout::BACKGROUND_IMAGE_ALIAS,
};

use super::StoreEditError;

#[derive(Debug)]
pub(super) struct ImageBackground {
	volume_name: String,
	volume_path: String,
	volume_created: u32,
	target_created: u32,
	parent_id: u32,
	target_id: u32,
}

impl ImageBackground {
	pub(super) fn admit(
		target: BackgroundAliasTarget,
		payload: &[u8],
	) -> Result<Self, StoreEditError> {
		let BackgroundAliasTarget {
			volume_name: Some(BackgroundAliasName::Unicode(volume_name)),
			target_path: Some(volume_path),
			volume_created: HfsCreationTime::Seconds(volume_created),
			target_created: HfsCreationTime::Seconds(target_created),
			parent_catalog_id: parent_id,
			target_catalog_id: target_id,
			..
		} = target
		else {
			return Err(StoreEditError::UnqualifiedBackgroundAlias);
		};
		let source = Self {
			volume_name,
			volume_path: String::from_utf8(volume_path)
				.map_err(|_| StoreEditError::UnqualifiedBackgroundAlias)?,
			volume_created,
			target_created,
			parent_id,
			target_id,
		};
		let properties = Value::from_reader(Cursor::new(payload)).map_err(ReadError::from)?;
		let encoded = properties
			.as_dictionary()
			.and_then(|properties| properties.get(BACKGROUND_IMAGE_ALIAS))
			.and_then(Value::as_data)
			.ok_or(StoreEditError::UnqualifiedBackgroundAlias)?;
		// The inspection report intentionally omits unrecognized alias tags. Full
		// byte equality qualifies all header fields, extras, ordering and padding.
		if alias_bytes(source.identity())? != encoded {
			return Err(StoreEditError::UnqualifiedBackgroundAlias);
		}
		Ok(source)
	}

	pub(super) fn identity(&self) -> ImageBackgroundIdentity<'_> {
		ImageBackgroundIdentity {
			volume_name: &self.volume_name,
			volume_created_hfs_seconds: self.volume_created,
			target_created_hfs_seconds: self.target_created,
			parent_catalog_id: self.parent_id,
			target_catalog_id: self.target_id,
			volume_relative_path: &self.volume_path,
		}
	}

	pub(super) fn retained_bytes(&self) -> usize {
		self.volume_name.capacity() + self.volume_path.capacity()
	}
}

pub(super) fn replace(
	payload: &[u8],
	identity: ImageBackgroundIdentity<'_>,
) -> Result<Vec<u8>, StoreEditError> {
	let new_alias = alias_bytes(identity)?;
	if new_alias.len() > BTreeMaster::PAGE_SIZE.as_usize() {
		return Err(StoreEditError::LeafBudget);
	}
	let mut properties = Value::from_reader(Cursor::new(payload)).map_err(ReadError::from)?;
	let slot = properties
		.as_dictionary_mut()
		.and_then(|properties| properties.get_mut(BACKGROUND_IMAGE_ALIAS))
		.ok_or(StoreEditError::MissingBackgroundAlias)?;
	*slot = Value::Data(new_alias);
	let mut bytes = Vec::new();
	properties
		.to_writer_binary(&mut bytes)
		.map_err(ReadError::from)?;
	Ok(bytes)
}

fn alias_bytes(identity: ImageBackgroundIdentity<'_>) -> Result<Vec<u8>, BackgroundAliasError> {
	build_image_background_alias(identity)?
		.to_bytes()
		.map_err(BackgroundAliasError::Encoding)
}
