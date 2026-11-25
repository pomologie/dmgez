use std::str;

use dmgez_finder::inspection::{
	BackgroundAlias, BackgroundAliasName, BackgroundAliasTarget, HfsCreationTime, RecordValue,
	Report, ViewKind,
};
use dmgez_vfs::{LogicalDisk, volume::NodeKind};
use hfsplus_reader::{Volume, catalog::CatalogLimits};

use crate::browse::{Error, HfsView, WindowSource};

use super::{Background, CaptureError, MAX_DEPTH, exact_child, read_fork};

const FOLDER_SETTINGS_NAME: &str = ".";
const IMAGE_BACKGROUND_TYPE: i64 = 2;
const MAX_ASSET_BYTES: usize = 16 << 20;
const MAX_ALIAS_PATH_BYTES: usize = 16 << 10;
const MAX_NAME_UNITS: usize = 255;
const PATH_SEPARATOR: char = '/';
const UNSUPPORTED_PATH_CHARACTERS: &[char] = &['\0', ':', '\\'];
const CREATION_DATE: usize = 0;
const ALIAS_TIMESTAMP_FRACTION_BITS: u32 = 16;

pub(super) fn capture<'disk, S: LogicalDisk>(
	view: &mut HfsView<'_, 'disk, S>,
	volume: &Volume<WindowSource<'disk, S>>,
	catalog_limits: CatalogLimits,
	report: &Report,
) -> Background {
	let settings = report
		.records
		.iter()
		.filter(|record| record.name == FOLDER_SETTINGS_NAME);
	if settings
		.clone()
		.any(|record| matches!(record.value, RecordValue::View(ViewKind::Other(_))))
	{
		return Background::Absent;
	}
	let alias = settings
		.filter_map(|record| match &record.value {
			RecordValue::IconView(view) if view.background_type == Some(IMAGE_BACKGROUND_TYPE) => {
				view.background_alias.as_ref()
			}
			_ => None,
		})
		.next();
	let target = match alias {
		None => return Background::Absent,
		Some(BackgroundAlias::File(target)) => target,
		Some(BackgroundAlias::Missing | BackgroundAlias::Malformed) => {
			return Background::InvalidReference;
		}
		_ => return Background::Unsupported,
	};
	match read_reference(view, volume, catalog_limits, target) {
		Ok(Some(bytes)) => Background::Encoded(bytes),
		Ok(None) => Background::InvalidReference,
		Err(error) => Background::Unavailable(error),
	}
}

fn read_reference<'disk, S: LogicalDisk>(
	view: &mut HfsView<'_, 'disk, S>,
	volume: &Volume<WindowSource<'disk, S>>,
	catalog_limits: CatalogLimits,
	alias: &BackgroundAliasTarget,
) -> Result<Option<Vec<u8>>, CaptureError> {
	if !matches_date(alias.volume_created, volume.header().dates[CREATION_DATE]) {
		return Ok(None);
	}
	let Some(parts) = components(alias.target_path.as_deref()) else {
		return Ok(None);
	};
	let Some((leaf, parents)) = parts.split_last() else {
		return Ok(None);
	};
	if !matches_name(alias.target_name.as_ref(), leaf) {
		return Ok(None);
	}
	let mut parent = view.root()?;
	for component in parents {
		let Some(entry) = exact_child(view, &parent, component)? else {
			return Ok(None);
		};
		if entry.kind != NodeKind::Directory {
			return Ok(None);
		}
		parent = entry.node;
	}
	let Some(entry) = exact_child(view, &parent, leaf)? else {
		return Ok(None);
	};
	if entry.kind != NodeKind::RegularFile
		|| parent.native_id().0 != alias.parent_catalog_id
		|| entry.node.native_id().0 != alias.target_catalog_id
		|| view.stat(&entry.node)?.hard_link.is_some()
	{
		return Ok(None);
	}

	// The generic view doesn't expose HFS dates or its root label. This
	// supplementary catalog read shares the already budgeted window source.
	let catalog = volume.open_catalog(catalog_limits).map_err(Error::from)?;
	let root = catalog.root().map_err(Error::from)?;
	if !matches_name(alias.volume_name.as_ref(), &root.name) {
		return Ok(None);
	}
	let Some(target) = catalog.entry(entry.node.native_id()).map_err(Error::from)? else {
		return Ok(None);
	};
	if target.parent != parent.native_id()
		|| target.name != *leaf
		|| !matches_date(alias.target_created, target.metadata.dates[CREATION_DATE])
	{
		return Ok(None);
	}
	read_fork(view, &entry.node, MAX_ASSET_BYTES).map(Some)
}

fn components(path: Option<&[u8]>) -> Option<Vec<Vec<u16>>> {
	let path = path.filter(|bytes| bytes.len() <= MAX_ALIAS_PATH_BYTES)?;
	let path = str::from_utf8(path).ok()?.strip_prefix(PATH_SEPARATOR)?;
	let mut parts = Vec::new();
	for component in path.split(PATH_SEPARATOR) {
		if component.is_empty()
			|| matches!(component, "." | "..")
			|| component.contains(UNSUPPORTED_PATH_CHARACTERS)
			|| parts.len() == MAX_DEPTH
		{
			return None;
		}
		let name: Vec<_> = component.encode_utf16().take(MAX_NAME_UNITS + 1).collect();
		if name.len() > MAX_NAME_UNITS {
			return None;
		}
		parts.push(name);
	}
	Some(parts)
}

fn matches_name(alias: Option<&BackgroundAliasName>, stored: &[u16]) -> bool {
	match alias {
		Some(BackgroundAliasName::Unicode(name)) => name.encode_utf16().eq(stored.iter().copied()),
		Some(BackgroundAliasName::Legacy(name)) if name.is_ascii() => name
			.iter()
			.copied()
			.map(u16::from)
			.eq(stored.iter().copied()),
		_ => false,
	}
}

fn matches_date(alias: HfsCreationTime, stored: u32) -> bool {
	match alias {
		HfsCreationTime::Seconds(seconds) => seconds == stored,
		HfsCreationTime::FixedPoint(time) => {
			time == u64::from(stored) << ALIAS_TIMESTAMP_FRACTION_BITS
		}
	}
}

#[cfg(test)]
mod tests;
