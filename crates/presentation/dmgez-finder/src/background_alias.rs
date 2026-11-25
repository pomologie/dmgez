use dmg_layout::{
	AliasHeader, AliasKind, AliasRecord, AliasTarget, AliasVolume, CatalogNodeId, DiskImageError,
	DiskType, ExtraEntry, FilesystemSignature, HfsTimestamp, HfsTimestampSeconds, VolumeAttributes,
	VolumeMountPath,
	layout::{TARGET_NAME_WIDTH, VOLUME_NAME_WIDTH},
};
use dmgez_config::{FinderOptions, VirtualPath};
use thiserror::Error;

use crate::store::{self, LayoutItem};

const PATH_SEPARATOR: char = '/';
const MACOS_VOLUME_MOUNT_DIRECTORY: &str = "/Volumes";
const CARBON_VOLUME_SEPARATOR: u8 = b':';
const CARBON_PATH_SEPARATOR: &[u8] = b":\0";
const INVALID_VOLUME_CHARACTERS: [char; 3] = [PATH_SEPARATOR, ':', '\0'];
const MAX_HFS_NAME_UNITS: usize = 255;
const HFS_ROOT_FOLDER_ID: u32 = 2;
const HFS_FIRST_USER_CATALOG_ID: u32 = 16;
const ALIAS_NO_CATALOG_ID: u32 = u32::MAX;
const MAX_LEGACY_ALIAS_BYTES: usize = i16::MAX as usize;
// The image has no mounted volume attributes. This matches mac_alias's
// version-2 file alias profile used by dmgbuild.
const NO_VOLUME_ATTRIBUTES: VolumeAttributes = VolumeAttributes(0);

/// An image background's identity from the final HFS+ catalog and volume.
/// Supply the catalog spelling of each name and the exact stored dates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageBackgroundIdentity<'a> {
	/// The HFS+ volume label.
	pub volume_name: &'a str,
	/// The stored volume creation date in seconds since the HFS epoch.
	pub volume_created_hfs_seconds: u32,
	/// The stored target creation date in seconds since the HFS epoch.
	pub target_created_hfs_seconds: u32,
	/// The Catalog Node ID of the target's containing folder.
	pub parent_catalog_id: u32,
	/// The Catalog Node ID of the generated image file.
	pub target_catalog_id: u32,
	/// The complete volume-relative file path with a leading slash.
	pub volume_relative_path: &'a str,
}

/// The supplied identity cannot be written as a Finder background alias.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BackgroundAliasError {
	/// The volume label cannot be used in an HFS+ file alias.
	#[error("invalid Finder background volume label")]
	VolumeName,
	/// The target path lacks a valid image file component.
	#[error("invalid Finder background file path")]
	TargetPath,
	/// The supplied catalog IDs are inconsistent with the target path.
	#[error("invalid catalog IDs for Finder background file and parent")]
	CatalogIds,
	/// The alias would exceed the signed 16-bit length used by legacy readers.
	#[error("Finder background alias exceeds {MAX_LEGACY_ALIAS_BYTES} bytes")]
	Size,
	/// The Alias codec rejected the resulting record.
	#[error("could not construct the Finder background alias")]
	Encoding(#[from] DiskImageError),
}

/// Builds a version-2 file Alias with a standard `/Volumes/<label>` mount hint.
/// Its Unicode names preserve the complete label and filename. A file one folder
/// below the root carries that folder's ID as its complete directory CNID trail.
/// Deeper files retain their full paths and direct parent CNID in the header.
///
/// # Errors
/// Returns an error for invalid catalog IDs, names or paths, or a legacy record
/// that cannot represent the supplied identity.
pub fn build_image_background_alias(
	identity: ImageBackgroundIdentity<'_>,
) -> Result<AliasRecord, BackgroundAliasError> {
	if !valid_hfs_name(identity.volume_name)
		|| identity.volume_name.contains(INVALID_VOLUME_CHARACTERS)
	{
		return Err(BackgroundAliasError::VolumeName);
	}
	if identity.volume_relative_path.len() > MAX_LEGACY_ALIAS_BYTES {
		return Err(BackgroundAliasError::Size);
	}
	let path: VirtualPath = identity
		.volume_relative_path
		.parse()
		.map_err(|_| BackgroundAliasError::TargetPath)?;
	if path.is_root() || !path.components().all(valid_hfs_name) {
		return Err(BackgroundAliasError::TargetPath);
	}
	let relative = &path.as_str()[PATH_SEPARATOR.len_utf8()..];
	let (parent_path, target_name) = relative
		.rsplit_once(PATH_SEPARATOR)
		.unwrap_or(("", relative));
	validate_catalog_ids(identity, parent_path.is_empty())?;
	let parent_name = parent_path
		.rsplit(PATH_SEPARATOR)
		.next()
		.filter(|name| !name.is_empty())
		.unwrap_or(identity.volume_name);

	let volume = AliasVolume::new(
		legacy_name(identity.volume_name, VOLUME_NAME_WIDTH),
		HfsTimestampSeconds(identity.volume_created_hfs_seconds),
		FilesystemSignature::HIERARCHICAL_FILE_SYSTEM_PLUS,
		DiskType::Fixed,
	)?;
	let target = AliasTarget::new(
		AliasKind::File,
		CatalogNodeId(identity.parent_catalog_id),
		legacy_name(target_name, TARGET_NAME_WIDTH),
		CatalogNodeId(identity.target_catalog_id),
		HfsTimestampSeconds(identity.target_created_hfs_seconds),
	)?;
	let mut header = AliasHeader::new(volume, target);
	header.volume_attributes = NO_VOLUME_ATTRIBUTES;
	let volume_path = VolumeMountPath::new(format!(
		"{MACOS_VOLUME_MOUNT_DIRECTORY}{PATH_SEPARATOR}{}",
		identity.volume_name
	))?;
	let mut extras = vec![
		ExtraEntry::carbon_folder_name(parent_name)?,
		ExtraEntry::high_resolution_volume_creation_date(alias_timestamp(
			identity.volume_created_hfs_seconds,
		))?,
		ExtraEntry::high_resolution_creation_date(alias_timestamp(
			identity.target_created_hfs_seconds,
		))?,
	];
	if !parent_path.is_empty() && !parent_path.contains(PATH_SEPARATOR) {
		extras.push(ExtraEntry::catalog_node_id_path(&[
			identity.parent_catalog_id
		])?);
	}
	extras.extend([
		ExtraEntry::carbon_path(carbon_path(identity.volume_name, relative))?,
		ExtraEntry::unicode_target_name(target_name)?,
		ExtraEntry::unicode_volume_name(identity.volume_name)?,
		ExtraEntry::portable_operating_system_interface_path(path.as_str())?,
		ExtraEntry::portable_operating_system_interface_volume_path(volume_path.as_bytes())?,
	]);
	let alias = AliasRecord::new(header, extras).map_err(|error| match error {
		DiskImageError::RecordTooLarge { .. } => BackgroundAliasError::Size,
		other => BackgroundAliasError::Encoding(other),
	})?;
	if alias.record_length() > MAX_LEGACY_ALIAS_BYTES {
		return Err(BackgroundAliasError::Size);
	}
	Ok(alias)
}

/// Writes a Finder store referencing the final on-volume background file.
///
/// # Errors
/// Returns an error if the identity cannot be encoded or the Finder settings,
/// icon positions, or resulting store exceed the supported writer profile.
pub fn write_store_with_background_identity(
	options: &FinderOptions,
	items: &[LayoutItem<'_>],
	identity: ImageBackgroundIdentity<'_>,
) -> Result<Vec<u8>, store::Error> {
	let alias = build_image_background_alias(identity)?;
	store::write_store_with_background_alias(options, items, &alias)
}

fn valid_hfs_name(name: &str) -> bool {
	!name.is_empty()
		&& name.encode_utf16().take(MAX_HFS_NAME_UNITS + 1).count() <= MAX_HFS_NAME_UNITS
}

fn legacy_name(name: &str, width: usize) -> &str {
	// The Pascal field reserves a byte for its length. Complete names are
	// available in the Unicode extras even if the prefix is shortened.
	&name[..name.floor_char_boundary(width - 1)]
}

fn validate_catalog_ids(
	identity: ImageBackgroundIdentity<'_>,
	parent_is_root: bool,
) -> Result<(), BackgroundAliasError> {
	let parent = identity.parent_catalog_id;
	let target = identity.target_catalog_id;
	let valid_parent = if parent_is_root {
		parent == HFS_ROOT_FOLDER_ID
	} else {
		(HFS_FIRST_USER_CATALOG_ID..ALIAS_NO_CATALOG_ID).contains(&parent)
	};
	if !valid_parent
		|| !(HFS_FIRST_USER_CATALOG_ID..ALIAS_NO_CATALOG_ID).contains(&target)
		|| parent == target
	{
		return Err(BackgroundAliasError::CatalogIds);
	}
	Ok(())
}

fn alias_timestamp(seconds: u32) -> HfsTimestamp {
	HfsTimestamp(u64::from(seconds) * HfsTimestamp::UNITS_PER_SECOND)
}

fn carbon_path(volume: &str, relative: &str) -> Vec<u8> {
	let mut path = volume.as_bytes().to_vec();
	path.push(CARBON_VOLUME_SEPARATOR);
	for (index, component) in relative.split(PATH_SEPARATOR).enumerate() {
		if index != 0 {
			path.extend_from_slice(CARBON_PATH_SEPARATOR);
		}
		path.extend_from_slice(component.as_bytes());
	}
	path
}
