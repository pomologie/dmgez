use dmgez_config::{AutoFlag, Project};
use dmgez_udif::writer::{Profile, WriteOptions};

use super::{
	PreflightError, PreflightOptions, ReleaseIntent, ResourceLimits, release::configured_release,
};

pub(super) fn admit(project: &Project, options: PreflightOptions) -> Result<(), PreflightError> {
	if options.release == ReleaseIntent::Release || configured_release(project) {
		return Err(PreflightError::UdrwRelease);
	}
	if project.build.verify_output {
		return Err(PreflightError::UdrwVerification);
	}
	if project.performance.parallel == AutoFlag::Enabled || project.performance.threads > 1 {
		return Err(PreflightError::UdrwParallel);
	}
	Ok(())
}

pub(super) fn writer_options(limits: ResourceLimits) -> WriteOptions {
	WriteOptions {
		profile: Profile::Udro,
		max_buffer_bytes: limits.memory_bytes - limits.max_metadata_bytes,
		..WriteOptions::default()
	}
}
