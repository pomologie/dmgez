use dmgez_config::Project;

use super::{
	BuildFeature, OutputIntent, PreflightError, PreflightOptions, ReleaseIntent, ReleasePlan,
};

pub(super) fn release_plan(
	project: &Project,
	options: PreflightOptions,
) -> Result<ReleasePlan, PreflightError> {
	if options.release == ReleaseIntent::Release {
		if options.output == OutputIntent::Stream {
			return Err(PreflightError::ReleaseStream);
		}
		options
			.capabilities
			.release
			.require(BuildFeature::Release)?;
		return Ok(ReleasePlan::Requested);
	}
	Ok(if configured_release(project) {
		ReleasePlan::ConfiguredStagesDeferred
	} else {
		ReleasePlan::Unsigned
	})
}

pub(super) fn configured_release(project: &Project) -> bool {
	project
		.signing
		.as_ref()
		.is_some_and(|stage| stage.enabled == Some(true))
		|| project
			.notarization
			.as_ref()
			.is_some_and(|stage| stage.enabled == Some(true))
		|| project.release.as_ref().is_some_and(|stage| {
			[
				stage.verify,
				stage.require_notarization,
				stage.require_native_assessment,
			]
			.contains(&Some(true))
		})
}
