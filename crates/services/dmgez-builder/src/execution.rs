//! Stream a sealed filesystem plan through the UDIF writer (C3 and C7).

use std::{
	io,
	io::{Read, Write},
	path::Path,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use dmgez_core::output::replace::{EntryIdentity, ReplaceError, TargetIdentity, publish_replace};
use dmgez_core::output::{PublishError, StagedReader, publish_new};
use dmgez_udif::writer::{WriteOptions, WriteReport, write_image};
use thiserror::Error;

use crate::{
	manifest::{self, Manifest},
	preflight::{ExecutionProfile, OutputPlan, PreflightError, ReleasePlan},
	progress::BuildProgress,
};

mod release_stage;

pub use release_stage::StagedRelease;

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
	#[error("a captured source failed its publication checks")]
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
	#[error("atomic replacement is unavailable on this host")]
	AtomicReplace,
	#[error("the admitted release plan requires an application-owned staging pipeline")]
	Release,
	#[error("prepublication verification was requested without an independent verifier")]
	VerifierRequired,
	#[error(transparent)]
	BeforeOutput(#[from] ImageError),
	#[error(transparent)]
	Path(#[from] PublishError<ImageError>),
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[error(transparent)]
	Replace(#[from] ReplaceError<ImageError>),
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
	/// rechecks captured source identities before publication. Overwrite
	/// requires an admitted replacement profile and an explicitly selected
	/// trusted local directory. Its advisory lock coordinates cooperating
	/// publishers. See the core replacement contract for filesystem assumptions.
	///
	/// # Errors
	/// Rejects a mismatched output target, unsupported replacement or release
	/// intent, changed sources, failed verification, cancellation and I/O errors.
	pub fn publish(
		self,
		destination: &Path,
		verifier: Option<&OutputVerifier<'_>>,
	) -> Result<BuildReport, ExecuteError> {
		self.publish_with_progress(destination, verifier, |_| {})
	}

	/// Publishes a path output and reports progress on the calling thread.
	///
	/// # Errors
	/// Returns the same failures as [`Self::publish`].
	pub fn publish_with_progress(
		self,
		destination: &Path,
		verifier: Option<&OutputVerifier<'_>>,
		progress: impl Fn(BuildProgress),
	) -> Result<BuildReport, ExecuteError> {
		let OutputPlan::Path { overwrite } = self.profile.output() else {
			return Err(ExecuteError::Target);
		};
		#[cfg(not(any(target_os = "linux", target_os = "macos")))]
		if overwrite {
			return Err(ExecuteError::AtomicReplace);
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
		let write = |stage: &mut dyn Write| {
			write_with_progress(
				self.reader,
				self.logical_bytes,
				stage,
				self.profile.write_options(),
				&progress,
			)
			.map_err(ImageError::from)
		};
		let validate = |stage: StagedReader<'_>, report: &WriteReport| {
			if let Some(verify) = verify {
				progress(BuildProgress::VerifyingOutput);
				check_cancellation(self.profile)?;
				verify(stage, report).map_err(ImageError::Verification)?;
			}
			progress(BuildProgress::Publishing);
			validate_sources(self.manifest, self.profile)
		};
		#[cfg(any(target_os = "linux", target_os = "macos"))]
		let image = if overwrite {
			publish_replace(
				destination,
				|target| authorize_replacement(self.manifest, self.profile, target),
				write,
				validate,
			)?
		} else {
			publish_new(destination, write, validate)?
		};
		#[cfg(not(any(target_os = "linux", target_os = "macos")))]
		let image = publish_new(destination, write, validate)?;
		progress(BuildProgress::Completed {
			physical_bytes: image.physical_bytes,
		});
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
		self.write_to_with_progress(sink, |_| {})
	}

	/// Writes a stream and reports progress on the calling thread.
	///
	/// # Errors
	/// Returns the same failures as [`Self::write_to`].
	pub fn write_to_with_progress(
		self,
		mut sink: impl Write,
		progress: impl Fn(BuildProgress),
	) -> Result<BuildReport, ExecuteError> {
		if self.profile.output() != OutputPlan::Stream {
			return Err(ExecuteError::Target);
		}
		validate_sources(self.manifest, self.profile)?;
		let image = write_with_progress(
			self.reader,
			self.logical_bytes,
			&mut sink,
			self.profile.write_options(),
			&progress,
		)
		.map_err(|error| ExecuteError::Stream(error.into()))?;
		validate_sources(self.manifest, self.profile).map_err(ExecuteError::Stream)?;
		sink.flush()
			.map_err(|error| ExecuteError::Stream(ImageError::Writer(error.into())))?;
		progress(BuildProgress::Completed {
			physical_bytes: image.physical_bytes,
		});
		Ok(BuildReport {
			image,
			release: self.profile.release(),
			verified: false,
		})
	}
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn authorize_replacement(
	manifest: &Manifest,
	profile: &ExecutionProfile,
	target: TargetIdentity,
) -> Result<(), ImageError> {
	let source_identity = |entry: EntryIdentity| manifest::SourceIdentity {
		device: entry.device,
		inode: entry.inode,
	};
	let destination = target.existing.map(source_identity);
	let directory = source_identity(target.directory);
	for source in manifest.sources() {
		check_cancellation(profile)?;
		let identity = source.metadata().identity;
		if destination == Some(identity) || (source.is_directory() && directory == identity) {
			return Err(manifest::Error::OutputSourceAlias {
				path: source.path().into(),
			}
			.into());
		}
	}
	check_cancellation(profile)
}

fn validate_sources(manifest: &Manifest, profile: &ExecutionProfile) -> Result<(), ImageError> {
	for source in manifest.sources() {
		check_cancellation(profile)?;
		source.validate()?;
	}
	check_cancellation(profile)
}

fn check_cancellation(profile: &ExecutionProfile) -> Result<(), ImageError> {
	if profile.write_options().cancellation.is_cancelled() {
		return Err(dmgez_udif::Error::Cancelled.into());
	}
	Ok(())
}

fn write_with_progress(
	reader: impl Read,
	logical_bytes: u64,
	sink: impl Write,
	options: &WriteOptions,
	progress: &dyn Fn(BuildProgress),
) -> Result<WriteReport, dmgez_udif::Error> {
	progress(BuildProgress::WritingImage { logical_bytes });
	let source = ObservedReader {
		reader,
		logical_bytes,
		logical_bytes_read: 0,
		chunk_bytes: options.chunk_bytes as u64,
		progress,
	};
	write_image(source, logical_bytes, sink, options)
}

struct ObservedReader<'observer, R> {
	reader: R,
	logical_bytes: u64,
	logical_bytes_read: u64,
	chunk_bytes: u64,
	progress: &'observer dyn Fn(BuildProgress),
}

impl<R: Read> Read for ObservedReader<'_, R> {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		let read = self.reader.read(buf)?;
		let previous = self.logical_bytes_read;
		self.logical_bytes_read = previous.saturating_add(read as u64);
		if self.logical_bytes_read < self.logical_bytes
			&& self.logical_bytes_read / self.chunk_bytes > previous / self.chunk_bytes
		{
			(self.progress)(BuildProgress::ReadingImage {
				logical_bytes_read: self.logical_bytes_read,
				logical_bytes: self.logical_bytes,
			});
		}
		Ok(read)
	}
}
