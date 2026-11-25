use dmgez_builder::{
	execution::{BuildPlan, OutputVerifier, StagedRelease},
	progress::BuildProgress,
};

use super::{Error, HfsImage};

impl<'build> HfsImage<'build> {
	/// Prepare an unsigned HFS+ image for the requested release workflow.
	/// The caller retains the stage through identity checks and release steps.
	/// # Errors
	/// Fails on an invalid release profile, changed inputs, failed verification,
	/// cancellation, or I/O.
	pub fn stage_release(
		self,
		verifier: &OutputVerifier<'_>,
		progress: impl Fn(BuildProgress),
	) -> Result<StagedRelease<'build>, Error> {
		let output = self.prepared.output_path().ok_or(Error::OutputPath)?;
		let build = BuildPlan::new(
			self.prepared.profile(),
			self.prepared.manifest(),
			self.image.reader(),
			self.logical_bytes(),
		)?;
		Ok(build.stage_release(output, verifier, progress)?)
	}
}
