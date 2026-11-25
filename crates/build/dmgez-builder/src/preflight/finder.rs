use dmgez_config::{Project, VolumeIcon};

#[cfg(feature = "finder-write")]
use super::{FilesystemProfile, filesystem::printable_ascii};
#[cfg(feature = "finder-write")]
use crate::presentation::{
	CatalogNames,
	catalog_paths::folder_key,
	settings::{resolve_options, without_positions},
};

use super::{
	Availability, BuildCapabilities, BuildFeature, FinderPlan, FinderSetting, PreflightError,
};

pub(super) fn finder_plan(
	project: &Project,
	capabilities: BuildCapabilities,
	#[cfg(feature = "finder-write")] filesystem: FilesystemProfile,
	#[cfg(feature = "finder-write")] names: CatalogNames,
) -> Result<FinderPlan, PreflightError> {
	if project
		.contents
		.iter()
		.any(|entry| entry.custom_icon.is_some())
	{
		if !cfg!(feature = "volume-icon") {
			return Err(PreflightError::FeatureNotCompiled(
				BuildFeature::FinderCustomIcon,
			));
		}
		capabilities
			.finder_custom_icon
			.require(BuildFeature::FinderCustomIcon)?;
	}
	if project
		.finder
		.as_ref()
		.is_some_and(|finder| !finder.hide.is_empty() || !finder.hide_extensions.is_empty())
		|| project
			.contents
			.iter()
			.any(|entry| entry.hidden.is_some() || entry.hide_extension.is_some())
	{
		capabilities
			.finder_flags
			.require(BuildFeature::FinderFlags)?;
	}
	let explicit = project.finder.is_some()
		|| project
			.contents
			.iter()
			.any(|entry| entry.position.is_some());
	let default_store =
		cfg!(feature = "finder-write") && capabilities.finder_layout == Availability::Available;
	if !explicit && !default_store {
		return Ok(FinderPlan::NoStore);
	}
	capabilities
		.finder_layout
		.require(BuildFeature::FinderLayout)?;
	admit_layout(
		project,
		capabilities.finder_image_background,
		#[cfg(feature = "finder-write")]
		filesystem,
		#[cfg(feature = "finder-write")]
		names,
	)
}

pub(super) fn admit_volume_icon(
	project: &Project,
	capabilities: BuildCapabilities,
) -> Result<Option<VolumeIcon>, PreflightError> {
	let Some(icon) = &project.volume_icon else {
		return Ok(None);
	};
	let (compiled, feature) = match icon {
		VolumeIcon::Source(_) | VolumeIcon::Images(_) => (
			cfg!(feature = "volume-icon"),
			BuildFeature::FinderVolumeIcon,
		),
		VolumeIcon::Badge(_) => (
			cfg!(feature = "badge-icons"),
			BuildFeature::FinderVolumeIconBadge,
		),
		_ => return Err(PreflightError::UnsupportedFinder(FinderSetting::VolumeIcon)),
	};
	if !compiled {
		return Err(PreflightError::FeatureNotCompiled(feature));
	}
	capabilities
		.finder_volume_icon
		.require(BuildFeature::FinderVolumeIcon)?;
	Ok(Some(icon.clone()))
}

#[cfg(not(feature = "finder-write"))]
fn admit_layout(
	_project: &Project,
	_image_background: Availability,
) -> Result<FinderPlan, PreflightError> {
	Err(PreflightError::FeatureNotCompiled(
		BuildFeature::FinderLayout,
	))
}

#[cfg(feature = "finder-write")]
fn admit_layout(
	project: &Project,
	image_background: Availability,
	filesystem: FilesystemProfile,
	names: CatalogNames,
) -> Result<FinderPlan, PreflightError> {
	use std::collections::BTreeSet;

	use dmgez_config::View;

	let Some(finder) = &project.finder else {
		return Ok(FinderPlan::IconView);
	};
	admit_options(Some(&finder.options), image_background)?;
	let inherited = (!finder.folders.is_empty()).then(|| without_positions(&finder.options));
	let mut folders = BTreeSet::new();
	for (index, folder) in finder.folders.iter().enumerate() {
		let supported = match filesystem {
			FilesystemProfile::HfsplusAscii => folder.path.components().all(printable_ascii),
			FilesystemProfile::HfsplusUnicode => folder
				.path
				.components()
				.all(|name| !name.chars().any(char::is_control)),
		};
		if !supported {
			return Err(PreflightError::FinderFolderName { index });
		}
		if !folders.insert(folder_key(folder.path.as_str(), names)?) {
			return Err(PreflightError::DuplicateFinderFolder { index });
		}
		let options = resolve_options(inherited.as_ref(), Some(&folder.options));
		admit_options(options.as_deref(), image_background)?;
	}
	check_duplicate_positions(project, finder, names)?;
	match finder.options.default_view {
		None | Some(View::IconView) => Ok(FinderPlan::IconView),
		Some(View::ListView) => Ok(FinderPlan::ListView),
		Some(View::ColumnView) => Ok(FinderPlan::ColumnView),
		Some(view) => Err(dmgez_finder::Error::View(view).into()),
	}
}

#[cfg(feature = "finder-write")]
fn admit_options(
	options: Option<&dmgez_config::FinderOptions>,
	image_background: Availability,
) -> Result<(), PreflightError> {
	if let Some(options) = options
		&& matches!(
			options.background,
			Some(
				dmgez_config::Background::Image { .. } | dmgez_config::Background::Template { .. }
			)
		) {
		image_background.require(BuildFeature::FinderImageBackground)?;
		dmgez_finder::validate_options_for_image_background(options)?;
	} else {
		dmgez_finder::validate_options(options)?;
	}
	Ok(())
}

#[cfg(feature = "finder-write")]
fn check_duplicate_positions(
	project: &Project,
	finder: &dmgez_config::Finder,
	names: CatalogNames,
) -> Result<(), PreflightError> {
	use std::collections::{BTreeMap, BTreeSet};

	const IMAGE_PATH_SEPARATOR: char = '/';
	let folders = std::iter::once(("", &finder.options)).chain(
		finder
			.folders
			.iter()
			.map(|folder| (folder.path.as_str(), &folder.options)),
	);
	let mut positions = BTreeMap::new();
	for (path, options) in folders {
		let Some(icon_view) = &options.icon_view else {
			continue;
		};
		let keys = icon_view
			.positions
			.keys()
			.map(|name| names.comparison_key(name))
			.collect::<Result<BTreeSet<_>, _>>()?;
		if keys.len() != icon_view.positions.len() {
			return Err(PreflightError::DuplicatePositionName);
		}
		positions.insert(folder_key(path, names)?, keys);
	}
	for (index, entry) in project
		.contents
		.iter()
		.enumerate()
		.filter(|(_, entry)| entry.position.is_some())
	{
		let (parent, name) = entry
			.destination
			.as_str()
			.rsplit_once(IMAGE_PATH_SEPARATOR)
			.expect("validated virtual paths begin with a separator");
		if let Some(keys) = positions.get(&folder_key(parent, names)?)
			&& keys.contains(&names.comparison_key(name)?)
		{
			return Err(PreflightError::DuplicatePosition { index });
		}
	}
	Ok(())
}
