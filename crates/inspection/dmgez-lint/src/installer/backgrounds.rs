use std::{io::ErrorKind, iter::once, path::Path};

use dmgez_builder::manifest::{self, AssetInspector, SourceSnapshot};
use dmgez_config::{Background, Project};

use super::{BackgroundScale, Budget, Error, Evidence, Limits, check_cancellation};

const ROOT_FOLDER: &str = "/";

enum Failure {
	Missing,
	InvalidSource,
}

pub(super) fn check(
	project: &Project,
	config_dir: &Path,
	limits: Limits,
	mut emit: impl FnMut(Evidence) -> Result<(), Error>,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<SourceSnapshot>, Error> {
	let Some(finder) = &project.finder else {
		return Ok(Vec::new());
	};
	let folders = once((ROOT_FOLDER, &finder.options)).chain(
		finder
			.folders
			.iter()
			.map(|folder| (folder.path.as_str(), &folder.options)),
	);
	let mut snapshots = Vec::new();
	let mut remaining = limits.max_backgrounds;
	let mut inspector = None;
	for (destination, options) in folders {
		check_cancellation(cancelled)?;
		let Some(Background::Image { source, retina }) = &options.background else {
			continue;
		};
		// An inherited image has the same source as its explicit declaration.
		let references = once((source, BackgroundScale::Base)).chain(
			retina
				.iter()
				.map(|source| (source, BackgroundScale::Retina)),
		);
		for (input, scale) in references {
			check_cancellation(cancelled)?;
			remaining = remaining.checked_sub(1).ok_or(Error::Limit {
				budget: Budget::Backgrounds,
				limit: limits.max_backgrounds as u64,
			})?;
			let inspector = match inspector {
				Some(ref inspector) => inspector,
				None => inspector.insert(AssetInspector::new(config_dir)?),
			};
			let failure = match inspector.capture_regular(input) {
				Ok(snapshot) => {
					snapshots.try_reserve(1)?;
					snapshots.push(snapshot);
					continue;
				}
				Err(manifest::Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
					Failure::Missing
				}
				Err(
					manifest::Error::RegularSourceRequired { .. }
					| manifest::Error::UnsupportedSource { .. },
				) => Failure::InvalidSource,
				Err(manifest::Error::Io { source, .. })
					if source.kind() == ErrorKind::NotADirectory =>
				{
					Failure::InvalidSource
				}
				Err(error) => return Err(error.into()),
			};
			let destination = destination.to_owned();
			let source = input.as_path().display().to_string();
			emit(match failure {
				Failure::Missing => Evidence::BackgroundMissing {
					destination,
					source,
					scale,
				},
				Failure::InvalidSource => Evidence::BackgroundInvalidSource {
					destination,
					source,
					scale,
				},
			})?;
		}
	}
	Ok(snapshots)
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use dmgez_config::Project;

	use super::{Budget, Error, Limits, check};

	const IMAGE_BACKGROUND_PROJECT: &str = r#"
schema_version = 1
[image]
name = "Installer"
output = "installer.dmg"

[finder]
background = { kind = "image", source = "missing.png" }
"#;

	#[test]
	fn background_count_is_checked_before_inspecting_the_source() {
		let project = Project::parse(IMAGE_BACKGROUND_PROJECT).unwrap();
		let limits = Limits {
			max_backgrounds: 0,
			..Limits::default()
		};
		let result = check(&project, Path::new("."), limits, |_| Ok(()), &mut || false);
		assert!(matches!(
			result,
			Err(Error::Limit {
				budget: Budget::Backgrounds,
				limit: 0
			})
		));
	}
}
