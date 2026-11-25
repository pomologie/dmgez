use dmgez_config::{LocalPath, Project, VolumeIcon};

use super::{
	Availability, BuildCapabilities, BuildFeature, FinderPlan, FinderSetting, PreflightError,
};

pub(super) fn finder_plan(
	project: &Project,
	capabilities: BuildCapabilities,
) -> Result<FinderPlan, PreflightError> {
	if project
		.contents
		.iter()
		.any(|entry| entry.custom_icon.is_some())
	{
		return Err(PreflightError::UnsupportedFinder(FinderSetting::CustomIcon));
	}
	if project
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
		cfg!(feature = "finder-write") && capabilities.finder_icon_view == Availability::Available;
	if !explicit && !default_store {
		return Ok(FinderPlan::NoStore);
	}
	capabilities
		.finder_icon_view
		.require(BuildFeature::FinderLayout)?;
	admit_layout(project)
}

pub(super) fn volume_icon_source(
	project: &Project,
	capabilities: BuildCapabilities,
) -> Result<Option<LocalPath>, PreflightError> {
	match &project.volume_icon {
		None => Ok(None),
		Some(VolumeIcon::Source(source)) => {
			if !cfg!(feature = "volume-icon") {
				return Err(PreflightError::FeatureNotCompiled(
					BuildFeature::FinderVolumeIcon,
				));
			}
			capabilities
				.finder_volume_icon
				.require(BuildFeature::FinderVolumeIcon)?;
			Ok(Some(source.clone()))
		}
		Some(_) => Err(PreflightError::UnsupportedFinder(FinderSetting::VolumeIcon)),
	}
}

#[cfg(not(feature = "finder-write"))]
fn admit_layout(_project: &Project) -> Result<FinderPlan, PreflightError> {
	Err(PreflightError::FeatureNotCompiled(
		BuildFeature::FinderLayout,
	))
}

#[cfg(feature = "finder-write")]
fn admit_layout(project: &Project) -> Result<FinderPlan, PreflightError> {
	use std::collections::BTreeSet;

	if let Some(finder) = &project.finder {
		dmgez_finder::validate_options(Some(&finder.options))?;
		let mut folders = BTreeSet::new();
		for (index, folder) in finder.folders.iter().enumerate() {
			if !folder.path.components().all(super::printable_ascii) {
				return Err(PreflightError::FinderFolderName { index });
			}
			if !folders.insert(folder.path.as_str().to_ascii_lowercase()) {
				return Err(PreflightError::DuplicateFinderFolder { index });
			}
			dmgez_finder::validate_options(Some(&folder.options))?;
		}
		check_duplicate_positions(project, finder)?;
	}
	Ok(FinderPlan::IconView)
}

#[cfg(feature = "finder-write")]
fn check_duplicate_positions(
	project: &Project,
	finder: &dmgez_config::Finder,
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
		let names: BTreeSet<_> = icon_view
			.positions
			.keys()
			.map(|name| name.to_ascii_lowercase())
			.collect();
		if names.len() != icon_view.positions.len() {
			return Err(PreflightError::DuplicatePositionName);
		}
		positions.insert(path.to_ascii_lowercase(), names);
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
		if positions
			.get(&parent.to_ascii_lowercase())
			.is_some_and(|names| names.contains(&name.to_ascii_lowercase()))
		{
			return Err(PreflightError::DuplicatePosition { index });
		}
	}
	Ok(())
}
