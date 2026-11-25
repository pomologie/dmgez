//! Read-only testing of installer layouts and requested security evidence.

use std::{
	fmt::Display,
	fs::{self, File, Metadata},
	io,
	num::NonZeroU32,
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf, absolute},
};

#[cfg(feature = "apfs-volume")]
use dmgez_apfs_read::{TransactionId, VolumeInfo, VolumeSelector};
use dmgez_operations::install::InstallerReport;
#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
use dmgez_operations::install::{InstallerLimits, check_installer};
use dmgez_udif::{
	Error as UdifError,
	reader::{CheckStatus, FileSource, Image, Limits, Verification},
};
#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use serde::Serialize;

mod security;

#[cfg(not(feature = "hfsplus-volume"))]
const HFSPLUS_READ_FEATURE: &str = "hfsplus-read";
#[cfg(not(feature = "apfs-volume"))]
const APFS_READ_FEATURE: &str = "apfs-read";

/// Additional native evidence to require after container and layout inspection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SecurityRequirements {
	/// Verify the static signature on the disk image.
	pub signature: bool,
	/// Validate the image's stapled ticket.
	pub ticket: bool,
	/// Require affirmative native policy assessment.
	pub native_assessment: bool,
}

/// Filesystem and optional native identifiers for installer layout inspection.
#[derive(Debug)]
#[non_exhaustive]
pub enum Filesystem {
	/// An HFS+ or HFSX volume.
	Hfsplus,
	/// An APFS volume, if the adapter is composed.
	Apfs {
		/// Parsed optional volume selector, or its input validation error.
		#[cfg(feature = "apfs-volume")]
		selector: Result<Option<VolumeSelector>, io::Error>,
		/// Exact retained checkpoint requested by the caller.
		#[cfg(feature = "apfs-volume")]
		checkpoint: Option<TransactionId>,
	},
}

/// Image checks and the evidence produced before any failure.
#[derive(Debug)]
pub struct Report {
	/// Absolute input path.
	pub artifact: PathBuf,
	/// Identity of the selected APFS volume, when observed.
	#[cfg(feature = "apfs-volume")]
	pub apfs_volume: Option<VolumeInfo>,
	/// Ordered installer test results.
	pub checks: Checks,
}

/// The source, container, layout, and requested security outcomes.
#[derive(Debug, Serialize)]
pub struct Checks {
	/// Continued identity of the source across all requested checks.
	pub source: Check,
	/// Container and checksums.
	pub container: Check<Checksums>,
	/// Installer layout.
	pub layout: Check<InstallerReport>,
	/// Static image signature.
	pub signature: Check<&'static str>,
	/// Stapled ticket.
	pub ticket: Check<&'static str>,
	/// Native policy assessment.
	pub native_assessment: Check<&'static str>,
}

impl Checks {
	/// Aggregate the outcome without discarding any individual check.
	#[must_use]
	pub fn outcome(&self) -> Outcome {
		let statuses = [
			self.source.status,
			self.container.status,
			self.layout.status,
			self.signature.status,
			self.ticket.status,
			self.native_assessment.status,
		];
		if statuses
			.iter()
			.any(|status| matches!(status, Status::Unavailable | Status::Error))
		{
			Outcome::Incomplete
		} else if statuses.contains(&Status::Failed) {
			Outcome::Failed
		} else if statuses.contains(&Status::Blocked) {
			Outcome::Incomplete
		} else {
			Outcome::Passed
		}
	}
}

/// Aggregate verdict for a complete installer test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
	/// All required checks passed.
	Passed,
	/// At least one check conclusively failed.
	Failed,
	/// At least one required check could not be completed.
	Incomplete,
}

/// Outcome of an individual installer test step.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
	/// The check produced affirmative evidence.
	Passed,
	/// The check produced a conclusive rejection.
	Failed,
	/// The backend or evidence was unavailable.
	Unavailable,
	/// A prerequisite prevented execution.
	Blocked,
	/// The caller did not request the check.
	Skipped,
	/// The check encountered an error.
	Error,
}

impl Status {
	/// Stable name used in human-readable and serialized output.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Passed => "passed",
			Self::Failed => "failed",
			Self::Unavailable => "unavailable",
			Self::Blocked => "blocked",
			Self::Skipped => "skipped",
			Self::Error => "error",
		}
	}
}

/// Verdict of an individual check, with evidence or diagnostics when available.
#[derive(Debug, Serialize)]
pub struct Check<T = ()> {
	/// Check outcome.
	pub status: Status,
	/// Diagnostic accompanying an unsuccessful check.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub detail: Option<String>,
	/// Evidence returned by a backend that completed the check.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub evidence: Option<T>,
}

impl<T> Check<T> {
	const fn new(status: Status) -> Self {
		Self {
			status,
			detail: None,
			evidence: None,
		}
	}

	fn detail(status: Status, detail: impl Display) -> Self {
		Self {
			detail: Some(detail.to_string()),
			..Self::new(status)
		}
	}

	fn evidence(status: Status, evidence: T) -> Self {
		Self {
			evidence: Some(evidence),
			..Self::new(status)
		}
	}

	const fn requested(required: bool) -> Self {
		Self::new(if required {
			Status::Blocked
		} else {
			Status::Skipped
		})
	}
}

/// Independent verification of the checksums required by the installer profile.
#[derive(Debug, Serialize)]
pub struct Checksums {
	/// Whether the data fork checksum passed.
	pub data_fork_verified: bool,
	/// Whether the master checksum passed.
	pub master_verified: bool,
	/// Number of block tables.
	pub tables: usize,
	/// Number of block tables with verified checksums.
	pub verified_tables: usize,
}

/// Tests a finished image and reports every check completed before any failure.
///
/// # Errors
/// Returns an I/O error if the input cannot be resolved to an absolute path.
/// Image validation and evidence errors appear in the report.
pub fn run(
	input: &Path,
	filesystem: Filesystem,
	partition: Option<NonZeroU32>,
	requirements: SecurityRequirements,
) -> io::Result<Report> {
	let mut report = Report {
		artifact: absolute(input)?,
		#[cfg(feature = "apfs-volume")]
		apfs_volume: None,
		checks: Checks {
			source: Check::new(Status::Blocked),
			container: Check::new(Status::Blocked),
			layout: Check::new(Status::Blocked),
			signature: Check::requested(requirements.signature),
			ticket: Check::requested(requirements.ticket),
			native_assessment: Check::requested(requirements.native_assessment),
		},
	};
	if let Err(error) = inspect(filesystem, partition, requirements, &mut report) {
		report.checks.source = Check::detail(Status::Error, error);
	}
	Ok(report)
}

fn inspect(
	filesystem: Filesystem,
	partition: Option<NonZeroU32>,
	requirements: SecurityRequirements,
	report: &mut Report,
) -> io::Result<()> {
	let path = &report.artifact;
	let source = Source::open(path)?;
	let checks = &mut report.checks;
	checks.source = Check::new(Status::Passed);
	let opened = Image::open_release(
		FileSource::new(source.file.try_clone()?).map_err(io::Error::other)?,
		Limits::default(),
	);
	let mut image = match opened {
		Ok(image) => image,
		Err(error) => {
			checks.container = container_error(&error);
			return source.validate();
		}
	};
	checks.container = match image.verify() {
		Ok(verification) => container_check(&verification),
		Err(error) => container_error(&error),
	};
	source.validate()?;
	if checks.container.status != Status::Passed {
		return Ok(());
	}
	checks.layout = match filesystem {
		Filesystem::Hfsplus => hfsplus(&mut image, partition),
		Filesystem::Apfs {
			#[cfg(feature = "apfs-volume")]
			selector,
			#[cfg(feature = "apfs-volume")]
			checkpoint,
		} => {
			#[cfg(feature = "apfs-volume")]
			{
				apfs(
					&mut image,
					partition,
					selector,
					checkpoint,
					&mut report.apfs_volume,
				)
			}
			#[cfg(not(feature = "apfs-volume"))]
			{
				Check::detail(Status::Unavailable, APFS_READ_FEATURE)
			}
		}
	};
	source.validate()?;
	if requirements.signature {
		checks.signature = security::signature(path);
		source.validate()?;
	}
	if requirements.ticket {
		checks.ticket = security::ticket(path);
		source.validate()?;
	}
	if requirements.native_assessment {
		checks.native_assessment = security::assess(path);
		source.validate()?;
	}
	Ok(())
}

fn container_check(verification: &Verification) -> Check<Checksums> {
	let checksums = Checksums {
		data_fork_verified: verification.data_fork == CheckStatus::Passed,
		master_verified: verification.master == CheckStatus::Passed,
		tables: verification.tables.len(),
		verified_tables: verification
			.tables
			.iter()
			.filter(|&&status| status == CheckStatus::Passed)
			.count(),
	};
	let status = if checksums.data_fork_verified
		&& checksums.master_verified
		&& checksums.verified_tables == checksums.tables
	{
		Status::Passed
	} else {
		Status::Unavailable
	};
	Check::evidence(status, checksums)
}

fn container_error(error: &UdifError) -> Check<Checksums> {
	let status = match error {
		UdifError::InvalidFormat(_)
		| UdifError::Corrupt(_)
		| UdifError::ChecksumMismatch { .. }
		| UdifError::InvalidZlib { .. }
		| UdifError::InvalidLzfse { .. }
		| UdifError::InvalidLzma { .. }
		| UdifError::Resource(_) => Status::Failed,
		UdifError::Unsupported(_)
		| UdifError::UnsupportedRun { .. }
		| UdifError::FeatureNotCompiled(_)
		| UdifError::ResourceLimit(_) => Status::Unavailable,
		_ => Status::Error,
	};
	Check::detail(status, error)
}

fn hfsplus(image: &mut Image<FileSource>, partition: Option<NonZeroU32>) -> Check<InstallerReport> {
	#[cfg(feature = "hfsplus-volume")]
	{
		layout_result(
			crate::volume::with_hfs_disk(image, partition, |view, _| check_layout(view))
				.map_err(io::Error::other)
				.and_then(std::convert::identity),
		)
	}
	#[cfg(not(feature = "hfsplus-volume"))]
	{
		let _ = (image, partition);
		Check::detail(Status::Unavailable, HFSPLUS_READ_FEATURE)
	}
}

#[cfg(feature = "apfs-volume")]
fn apfs(
	image: &mut Image<FileSource>,
	partition: Option<NonZeroU32>,
	selector: Result<Option<VolumeSelector>, io::Error>,
	checkpoint: Option<TransactionId>,
	identity: &mut Option<VolumeInfo>,
) -> Check<InstallerReport> {
	use crate::volume::{require_apfs_checkpoint, with_apfs_disk};

	layout_result((|| {
		let selector = selector?;
		with_apfs_disk(image, partition, checkpoint, |selected, container| {
			require_apfs_checkpoint(checkpoint, container.diagnostics())
				.map_err(io::Error::other)?;
			let backend = container.open_volume(selector).map_err(io::Error::other)?;
			*identity = Some(backend.info());
			check_layout(&mut VolumeView::new(selected, backend))
		})
		.map_err(io::Error::other)?
	})())
}

#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
fn check_layout<B: VolumeBackend>(view: &mut VolumeView<B>) -> io::Result<InstallerReport>
where
	B::Id: Eq + std::hash::Hash,
	B::Error: Send + Sync,
{
	check_installer(view, InstallerLimits::default(), || false).map_err(io::Error::other)
}

#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
fn layout_result(result: io::Result<InstallerReport>) -> Check<InstallerReport> {
	match result {
		Ok(report) => {
			let status = if report.is_valid() {
				Status::Passed
			} else {
				Status::Failed
			};
			Check::evidence(status, report)
		}
		Err(error) => Check::detail(Status::Error, error),
	}
}

struct Source<'path> {
	path: &'path Path,
	file: File,
	initial: Metadata,
}

impl<'path> Source<'path> {
	fn open(path: &'path Path) -> io::Result<Self> {
		let initial = fs::symlink_metadata(path)?;
		if !initial.is_file() {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"installer image must be a regular file",
			));
		}
		let source = Self {
			path,
			file: File::open(path)?,
			initial,
		};
		source.validate()?;
		Ok(source)
	}

	fn validate(&self) -> io::Result<()> {
		for observed in [self.file.metadata()?, fs::symlink_metadata(self.path)?] {
			if !same_file(&self.initial, &observed) {
				return Err(io::Error::new(
					io::ErrorKind::InvalidData,
					"installer image changed during testing",
				));
			}
		}
		Ok(())
	}
}

fn same_file(expected: &Metadata, observed: &Metadata) -> bool {
	observed.is_file()
		&& expected.dev() == observed.dev()
		&& expected.ino() == observed.ino()
		&& expected.len() == observed.len()
		&& expected.mtime() == observed.mtime()
		&& expected.mtime_nsec() == observed.mtime_nsec()
		&& expected.ctime() == observed.ctime()
		&& expected.ctime_nsec() == observed.ctime_nsec()
}
