//! Read-only analysis and verification workflows over admitted image sources.

use std::{
	error::Error as StdError,
	num::{NonZeroU32, NonZeroU64},
};

use dmgez_inspector::{AnalysisReport, VerificationMode, VerificationReport};
use dmgez_udif::{
	Error,
	reader::{Limits, ReadAt},
};

use crate::image::{ImageSession, MetadataBudget};

#[cfg(feature = "apfs-volume")]
mod apfs;
#[cfg(feature = "apfs-volume")]
pub use apfs::verify_apfs;
#[cfg(feature = "hfsplus-analysis")]
mod hfsplus_analysis;
#[cfg(feature = "hfsplus-analysis")]
pub use hfsplus_analysis::{HfsAnalysis, HfsAnalysisError, analyze_hfsplus};
#[cfg(feature = "hfsplus-verification")]
mod hfsplus_verify;

/// Highest supported filesystem payload budget, in bytes.
pub const MAX_FILESYSTEM_PAYLOAD_BYTES: u64 = 1 << 30;

/// Bound on stored HFS+ or logical APFS payload bytes read by verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadBudget(u64);

impl PayloadBudget {
	/// Selects a positive budget within the supported range.
	#[must_use]
	pub const fn new(bytes: NonZeroU64) -> Option<Self> {
		if bytes.get() <= MAX_FILESYSTEM_PAYLOAD_BYTES {
			Some(Self(bytes.get()))
		} else {
			None
		}
	}

	/// Returns the maximum number of bytes permitted by this budget.
	#[must_use]
	pub const fn bytes(self) -> u64 {
		self.0
	}
}

impl Default for PayloadBudget {
	fn default() -> Self {
		Self(MAX_FILESYSTEM_PAYLOAD_BYTES)
	}
}

/// Structured verification coverage with any underlying filesystem failure.
#[derive(Debug)]
#[non_exhaustive]
pub struct Verification {
	pub report: VerificationReport,
	pub filesystem_cause: Option<Box<dyn StdError>>,
}

/// Reports container and run allocation from an admitted image.
///
/// # Errors
/// Returns errors from container admission or revalidation of the held source.
pub fn analyze(source: impl ReadAt) -> Result<AnalysisReport, Error> {
	ImageSession::open(source, Limits::default(), MetadataBudget::ReaderLimits)?.analyze()
}

/// Checks a UDIF image without a filesystem checker.
///
/// A full request reports that filesystem verification is unavailable.
#[must_use]
pub fn verify_container(source: impl ReadAt, mode: VerificationMode) -> Verification {
	Verification {
		report: dmgez_inspector::verify(source, Limits::default(), mode),
		filesystem_cause: None,
	}
}

/// Checks a UDIF image and, for a full request with HFS+ verification support
/// compiled in, checks the selected HFS+ or HFSX window.
#[must_use]
pub fn verify_hfsplus(
	source: impl ReadAt,
	mode: VerificationMode,
	partition: Option<NonZeroU32>,
	budget: PayloadBudget,
) -> Verification {
	#[cfg(feature = "hfsplus-verification")]
	if mode == VerificationMode::Full {
		return hfsplus_verify::verify(source, partition, budget.bytes());
	}
	#[cfg(not(feature = "hfsplus-verification"))]
	let _ = (partition, budget);

	let mut evidence = verify_container(source, mode);
	evidence.report.capabilities.filesystem = cfg!(feature = "hfsplus-verification");
	evidence
}
