use std::{
	io::{self, Read},
	path::Path,
};

use dmgez_core::output::{StagedOutput, StagedReader, stage_new};
use dmgez_udif::writer::WriteReport;

use super::{
	BuildPlan, ExecuteError, ImageError, OutputVerifier, check_cancellation, validate_sources,
	write_with_progress,
};
use crate::{
	manifest::Manifest,
	preflight::{ExecutionProfile, OutputPlan, ReleasePlan},
	progress::BuildProgress,
};

const IMAGE_SUFFIX: &str = ".dmg";

/// A synced, independently verified unsigned image for the release composer.
pub struct StagedRelease<'plan> {
	output: StagedOutput<WriteReport>,
	manifest: &'plan Manifest,
	profile: &'plan ExecutionProfile,
}

impl StagedRelease<'_> {
	/// Absolute or caller-relative staging path in the trusted output directory.
	#[must_use]
	pub fn path(&self) -> &Path {
		self.output.path()
	}

	/// Geometry and checksums of the verified unsigned stage.
	#[must_use]
	pub const fn report(&self) -> &WriteReport {
		self.output.report()
	}

	/// Read the held unsigned file from byte zero.
	///
	/// # Errors
	/// Returns a file positioning error.
	pub fn reader(&mut self) -> io::Result<StagedReader<'_>> {
		self.output.reader()
	}

	/// Recheck the input manifest before a later release transition.
	///
	/// # Errors
	/// Returns an error for changed sources, cancellation, or failed I/O.
	pub fn validate_sources(&self) -> Result<(), ImageError> {
		validate_sources(self.manifest, self.profile)
	}
}

impl<'plan, R: Read> BuildPlan<'plan, R> {
	/// Prepare an unsigned image for the release composer to verify and sign.
	///
	/// The destination determines the staging directory. The returned owner
	/// removes the stage on drop. The caller owns all further release steps,
	/// including exact artifact identity checks and final publication.
	///
	/// # Errors
	/// Returns an error for a mismatched output or release intent, changed
	/// sources, cancellation, failed verification or I/O.
	pub fn stage_release(
		self,
		destination: &Path,
		verify: &OutputVerifier<'_>,
		progress: impl Fn(BuildProgress),
	) -> Result<StagedRelease<'plan>, ExecuteError> {
		if !matches!(self.profile.output(), OutputPlan::Path { .. }) {
			return Err(ExecuteError::Target);
		}
		if self.profile.release() != ReleasePlan::Requested {
			return Err(ExecuteError::Release);
		}
		validate_sources(self.manifest, self.profile)?;
		let output = stage_new(
			destination,
			IMAGE_SUFFIX,
			|stage| {
				write_with_progress(
					self.reader,
					self.logical_bytes,
					stage,
					self.profile.write_options(),
					&progress,
					#[cfg(feature = "compression-cache")]
					None,
				)
				.map(|output| output.image)
				.map_err(ImageError::from)
			},
			|stage, report| {
				progress(BuildProgress::VerifyingOutput);
				check_cancellation(self.profile)?;
				verify(stage, report).map_err(ImageError::Verification)?;
				validate_sources(self.manifest, self.profile)
			},
		)?;
		Ok(StagedRelease {
			output,
			manifest: self.manifest,
			profile: self.profile,
		})
	}
}
