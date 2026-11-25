//! Filesystem naming admission based on the selected builder profile.

use std::ops::RangeInclusive;

use dmgez_config::Project;

use super::{Availability, BuildCapabilities, BuildFeature, FilesystemProfile, PreflightError};

const PRINTABLE_ASCII: RangeInclusive<u8> = b' '..=b'~';

pub(super) fn filesystem_profile(
	capabilities: BuildCapabilities,
) -> Result<FilesystemProfile, PreflightError> {
	match (capabilities.hfsplus_unicode, capabilities.hfsplus_ascii) {
		(Availability::Available, _) => Ok(FilesystemProfile::HfsplusUnicode),
		(_, Availability::Available) => Ok(FilesystemProfile::HfsplusAscii),
		(Availability::Unavailable, _) | (_, Availability::Unavailable) => {
			Err(PreflightError::BackendUnavailable(BuildFeature::Hfsplus))
		}
		_ => Err(PreflightError::FeatureNotCompiled(BuildFeature::Hfsplus)),
	}
}

pub(super) fn check_known_names(project: &Project) -> Result<(), PreflightError> {
	if !printable_ascii(project.image.name.as_str()) {
		return Err(PreflightError::VolumeLabel);
	}
	for (index, entry) in project.contents.iter().enumerate() {
		if !entry.destination.components().all(printable_ascii) {
			return Err(PreflightError::Destination { index });
		}
	}
	Ok(())
}

pub(super) fn printable_ascii(name: &str) -> bool {
	name.bytes().all(|byte| PRINTABLE_ASCII.contains(&byte))
}
