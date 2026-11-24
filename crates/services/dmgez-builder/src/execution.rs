//! Stream a sealed filesystem plan through the UDIF writer (C3 and C7).

use std::{
	io,
	io::{Read, Write},
	path::Path,
};

use dmgez_core::output::{PublishError, StagedReader, publish_new};
use dmgez_udif::writer::{WriteReport, write_image};
use thiserror::Error;

use crate::{
	manifest::{self, Manifest},
	preflight::{ExecutionProfile, OutputPlan, PreflightError, ReleasePlan},
};

/// An admitted build backed by a sealed logical filesystem image.
/// The reader must expose the image from byte zero and end at `logical_bytes`.
pub struct BuildPlan<'plan, R> {
	profile: &'plan ExecutionProfile,
	manifest: &'plan Manifest,
	reader: R,
	logical_bytes: u64,
}

/// Successful publication or stream completion.
#[derive(Debug)]
#[non_exhaustive]
pub struct BuildReport {
	/// Final UDIF geometry and checksums.
	pub image: WriteReport,
	/// Whether configured release stages were deferred.
	pub release: ReleasePlan,
	/// Whether prepublication verification was requested and completed.
	pub verified: bool,
}

/// Independent verifier supplied by the composer for a synced staged image.
pub type OutputVerifier<'context> =
	dyn for<'file> Fn(StagedReader<'file>, &WriteReport) -> io::Result<()> + 'context;

/// An error writing or checking a filesystem plan.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImageError {
	#[error(transparent)]
	Writer(#[from] dmgez_udif::Error),
	#[error("a captured source failed its final identity check")]
	Source(#[from] manifest::Error),
	#[error("independent staged image verification failed")]
	Verification(#[source] io::Error),
}

/// A failure before output publication or complete stream emission.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecuteError {
	#[error("output target does not match the admitted profile")]
	Target,
	#[error("atomic replacement requires a composer with source alias protection")]
	AtomicReplace,
	#[error("the admitted release plan requires an application-owned staging pipeline")]
	Release,
	#[error("prepublication verification was requested without an independent verifier")]
	VerifierRequired,
	#[error(transparent)]
	BeforeOutput(#[from] ImageError),
	#[error(transparent)]
	Path(#[from] PublishError<ImageError>),
	#[error("output stream may be incomplete")]
	Stream(#[source] ImageError),
}

impl<'plan, R: Read> BuildPlan<'plan, R> {
	/// Admits a sealed image's logical capacity and run count.
	/// The caller supplies the manifest used to create the filesystem plan.
	///
	/// # Errors
	/// Rejects a logical size outside the configured capacity or writer budgets.
	pub fn new(
		profile: &'plan ExecutionProfile,
		manifest: &'plan Manifest,
		reader: R,
		logical_bytes: u64,
	) -> Result<Self, PreflightError> {
		profile.validate_logical_bytes(logical_bytes)?;
		Ok(Self {
			profile,
			manifest,
			reader,
			logical_bytes,
		})
	}

	/// Writes and syncs a same-directory stage, verifies it if requested, and
	/// checks captured source identities before a no-clobber commit.
	///
	/// # Errors
	/// Rejects a mismatched output target, unsupported replacement or release
	/// intent, changed sources, failed verification, cancellation and I/O errors.
	pub fn publish(
		self,
		destination: &Path,
		verifier: Option<&OutputVerifier<'_>>,
	) -> Result<BuildReport, ExecuteError> {
		match self.profile.output() {
			OutputPlan::Stream => return Err(ExecuteError::Target),
			OutputPlan::Path { overwrite: true } => return Err(ExecuteError::AtomicReplace),
			OutputPlan::Path { overwrite: false } => {}
		}
		let verified = self.profile.verify_output();
		let verify = if verified {
			Some(verifier.ok_or(ExecuteError::VerifierRequired)?)
		} else {
			None
		};
		let release = self.profile.release();
		if release == ReleasePlan::Requested {
			return Err(ExecuteError::Release);
		}
		validate_sources(self.manifest, self.profile)?;
		let image = publish_new(
			destination,
			|stage| {
				write_image(
					self.reader,
					self.logical_bytes,
					stage,
					self.profile.write_options(),
				)
				.map_err(ImageError::from)
			},
			|stage, report| {
				if let Some(verify) = verify {
					verify(stage, report).map_err(ImageError::Verification)?;
				}
				validate_sources(self.manifest, self.profile)
			},
		)?;
		Ok(BuildReport {
			image,
			release,
			verified,
		})
	}

	/// Writes to an admitted caller-owned stream.
	///
	/// # Errors
	/// Returns a failure if the target kind differs from the admitted intent,
	/// a source changed, cancellation was requested, or I/O failed. Once
	/// emission starts the caller must discard the stream on any error.
	pub fn write_to(self, sink: impl Write) -> Result<BuildReport, ExecuteError> {
		if self.profile.output() != OutputPlan::Stream {
			return Err(ExecuteError::Target);
		}
		validate_sources(self.manifest, self.profile)?;
		let image = write_image(
			self.reader,
			self.logical_bytes,
			sink,
			self.profile.write_options(),
		)
		.map_err(|error| ExecuteError::Stream(error.into()))?;
		validate_sources(self.manifest, self.profile).map_err(ExecuteError::Stream)?;
		Ok(BuildReport {
			image,
			release: self.profile.release(),
			verified: false,
		})
	}
}

fn validate_sources(manifest: &Manifest, profile: &ExecutionProfile) -> Result<(), ImageError> {
	let cancellation = &profile.write_options().cancellation;
	for entry in manifest.entries() {
		if cancellation.is_cancelled() {
			return Err(dmgez_udif::Error::Cancelled.into());
		}
		if let Some(source) = &entry.source {
			source.validate()?;
		}
	}
	if cancellation.is_cancelled() {
		return Err(dmgez_udif::Error::Cancelled.into());
	}
	Ok(())
}
