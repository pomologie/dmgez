use dmgez_udif::{
	Error,
	reader::{
		CheckStatus as UdifStatus, Checksum, Descriptor, Image, Limits, ReadAt, Verification,
	},
};
use serde::Serialize;

use crate::{
	Capabilities, CheckStatus, ContainerSummary, Coverage, Diagnostic, ErrorCategory,
	FilesystemCheckError, FilesystemCheckReport, FilesystemEvidence, FilesystemOptions,
	FilesystemSource, ReportOperation, SCHEMA_VERSION, Stage, filesystem::verify_window,
	schema::CAPABILITIES,
};

/// Work requested by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum VerificationMode {
	/// Open trailer and block-map metadata and admit the logical ranges.
	Quick,
	/// Decode stored runs and verify all declared container checksum domains.
	Container,
	/// Verify the container and filesystem. Without an explicit filesystem
	/// checker, successful container checks produce a partial result.
	Full,
}

/// Overall result for the requested verification scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
	Passed,
	Partial,
	Failed,
	Unsupported,
	Unavailable,
}

/// Per-domain checksum status in admitted block-map resource order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ChecksumChecks {
	pub data_fork: CheckStatus,
	pub tables: Vec<CheckStatus>,
	pub master: CheckStatus,
}

impl From<&Descriptor> for ChecksumChecks {
	fn from(descriptor: &Descriptor) -> Self {
		Self {
			data_fork: declaration_status(descriptor.physical_crc),
			tables: descriptor
				.tables
				.iter()
				.map(|table| declaration_status(table.checksum))
				.collect(),
			master: declaration_status(descriptor.master_crc),
		}
	}
}

impl From<Verification> for ChecksumChecks {
	fn from(verification: Verification) -> Self {
		Self {
			data_fork: verification.data_fork.into(),
			tables: verification
				.tables
				.into_iter()
				.map(CheckStatus::from)
				.collect(),
			master: verification.master.into(),
		}
	}
}

impl From<UdifStatus> for CheckStatus {
	fn from(status: UdifStatus) -> Self {
		match status {
			UdifStatus::Passed => Self::Passed,
			UdifStatus::NotPresent => Self::NotPresent,
		}
	}
}

const fn declaration_status(declaration: Checksum) -> CheckStatus {
	match declaration {
		Checksum::Absent => CheckStatus::NotPresent,
		Checksum::Crc32(_) => CheckStatus::NotChecked,
		Checksum::Unsupported { .. } => CheckStatus::Unsupported,
	}
}

/// Verification report, including failures to admit container metadata.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct VerificationReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	pub mode: VerificationMode,
	pub outcome: VerificationOutcome,
	pub capabilities: Capabilities,
	pub coverage: Coverage,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub container: Option<ContainerSummary>,
	/// On a stopped scan, the per-domain results retain declaration statuses.
	/// The underlying reader returns the first error without a domain ledger.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub checksums: Option<ChecksumChecks>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub filesystem_check: Option<FilesystemCheckReport>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub diagnostic: Option<Diagnostic>,
	#[serde(skip)]
	cause: Option<Error>,
}

impl VerificationReport {
	/// Whether the entire requested scope passed.
	#[must_use]
	pub const fn passed(&self) -> bool {
		matches!(self.outcome, VerificationOutcome::Passed)
	}
	/// Local causal error for callers that need the underlying typed information.
	#[must_use]
	pub const fn cause(&self) -> Option<&Error> {
		self.cause.as_ref()
	}

	fn fail(&mut self, stage: Stage, error: Error) {
		let diagnostic = Diagnostic::from_udif(stage, &error);
		self.fail_with(diagnostic, Some(error));
	}

	fn fail_with(&mut self, diagnostic: Diagnostic, cause: Option<Error>) {
		let status = diagnostic.category.status();
		match diagnostic.stage {
			Stage::Structure => self.coverage.structure = status,
			Stage::Payload => self.coverage.payload = status,
			Stage::Filesystem => self.coverage.filesystem = status,
		}
		self.outcome = match status {
			CheckStatus::Failed => VerificationOutcome::Failed,
			_ if self.coverage.structure == CheckStatus::Passed => VerificationOutcome::Partial,
			CheckStatus::Unsupported => VerificationOutcome::Unsupported,
			_ => VerificationOutcome::Unavailable,
		};
		self.diagnostic = Some(diagnostic);
		self.cause = cause;
	}
}

/// Opens the immutable source and checks the requested verification scope.
///
/// Container mode checks stored runs and every declared checksum. Omitted
/// checksum declarations remain `not_present`, even after a successful scan.
/// A full request requires filesystem checks and remains partial without them.
#[must_use]
pub fn verify(source: impl ReadAt, limits: Limits, mode: VerificationMode) -> VerificationReport {
	let (mut report, next) = verify_container(source, limits, mode);
	if next.is_some() && mode == VerificationMode::Full {
		report.fail_with(
			Diagnostic {
				stage: Stage::Filesystem,
				category: ErrorCategory::FeatureNotCompiled,
				detail: None,
			},
			None,
		);
	}
	report
}

/// Runs full verification with a filesystem checker for one admitted data window.
/// The checker is called once, after the container checks and window selection
/// pass. Reachable or structural checks, checks of one native subvolume, and
/// checks of one among multiple data windows leave whole-image coverage partial.
/// Security checks remain independent of this operation.
#[must_use]
pub fn verify_filesystem(
	source: impl ReadAt,
	limits: Limits,
	filesystem: FilesystemOptions,
	checker: impl FnOnce(&mut dyn FilesystemSource) -> Result<FilesystemEvidence, FilesystemCheckError>,
) -> VerificationReport {
	let (mut report, next) = verify_container(source, limits, VerificationMode::Full);
	report.capabilities.filesystem = true;
	let Some(mut image) = next else {
		return report;
	};
	match verify_window(&mut image, filesystem, checker) {
		Ok(attempt) => {
			report.filesystem_check = Some(attempt.report);
			if let Some(failure) = attempt.failure {
				report.fail_with(failure.diagnostic, failure.cause);
			} else if attempt.report.all_filesystems_checked {
				report.coverage.filesystem = CheckStatus::Passed;
				report.outcome = VerificationOutcome::Passed;
			} else {
				report.coverage.filesystem = CheckStatus::Partial;
				report.outcome = VerificationOutcome::Partial;
			}
		}
		Err(failure) => report.fail_with(failure.diagnostic, failure.cause),
	}
	report
}

fn verify_container<S: ReadAt>(
	source: S,
	limits: Limits,
	mode: VerificationMode,
) -> (VerificationReport, Option<Image<S>>) {
	let mut report = VerificationReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Verify,
		mode,
		outcome: VerificationOutcome::Unavailable,
		capabilities: CAPABILITIES,
		coverage: Coverage::default(),
		container: None,
		checksums: None,
		filesystem_check: None,
		diagnostic: None,
		cause: None,
	};
	let image = source
		.check_unchanged()
		.and_then(|()| Image::open(source, limits));
	let mut image = match image.and_then(|mut image| {
		// An empty checked read validates the backing source without a payload read.
		image.read_exact_at(0, &mut [])?;
		Ok(image)
	}) {
		Ok(image) => image,
		Err(error) => {
			report.fail(Stage::Structure, error);
			return (report, None);
		}
	};
	report.coverage.structure = CheckStatus::Passed;
	report.container = Some(ContainerSummary::read(&image));
	report.checksums = Some(image.descriptor().into());

	if mode == VerificationMode::Quick {
		report.outcome = VerificationOutcome::Passed;
		return (report, None);
	}
	match image.verify().and_then(|verified| {
		image.read_exact_at(0, &mut [])?;
		Ok(verified)
	}) {
		Ok(verified) => {
			report.checksums = Some(verified.into());
			report.coverage.payload = CheckStatus::Passed;
		}
		Err(error) => {
			report.fail(Stage::Payload, error);
			return (report, None);
		}
	}

	report.outcome = VerificationOutcome::Passed;
	(report, (mode == VerificationMode::Full).then_some(image))
}
