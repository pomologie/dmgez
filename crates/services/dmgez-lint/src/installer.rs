//! Source-aware static checks for a resolved installer project.

use std::{collections::TryReserveError, io, path::Path};

use dmgez_builder::manifest::{self, Limits as ManifestLimits};
use dmgez_config::Project;
use thiserror::Error;

mod backgrounds;
mod bundles;
mod paths;
mod report;

use report::SCHEMA_VERSION;
pub use report::{
	BackgroundScale, BundleInfoProblem, Diagnostic, Evidence, ParseRuleIdError, Report, RuleId,
};

const DEFAULT_MAX_DIAGNOSTICS: usize = 10_000;
const DEFAULT_MAX_PLIST_BYTES: u64 = 1 << 20;
const DEFAULT_MAX_TOTAL_PLIST_BYTES: u64 = 32 << 20;
const DEFAULT_MAX_BACKGROUNDS: usize = 4_096;

/// Stable CLI and report identifier for this profile.
pub const PROFILE_NAME: &str = "installer";

/// Installer rule suppression and its exact caller-provided reason.
#[derive(Clone, Copy, Debug)]
pub struct Suppression<'input> {
	rule: RuleId,
	reason: &'input str,
}

impl<'input> Suppression<'input> {
	/// Creates one rule-wide suppression.
	///
	/// # Errors
	/// Returns an error if the reason is empty or whitespace-only.
	pub fn new(rule: RuleId, reason: &'input str) -> Result<Self, Error> {
		if reason.trim().is_empty() {
			return Err(Error::SuppressionReason(rule));
		}
		Ok(Self { rule, reason })
	}
}

/// Independent limits for discovery, source reads, and retained diagnostics.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	pub manifest: ManifestLimits,
	pub max_diagnostics: usize,
	pub max_plist_bytes: u64,
	pub max_total_plist_bytes: u64,
	/// Maximum explicitly declared image references, including Retina variants.
	pub max_backgrounds: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			manifest: ManifestLimits::default(),
			max_diagnostics: DEFAULT_MAX_DIAGNOSTICS,
			max_plist_bytes: DEFAULT_MAX_PLIST_BYTES,
			max_total_plist_bytes: DEFAULT_MAX_TOTAL_PLIST_BYTES,
			max_backgrounds: DEFAULT_MAX_BACKGROUNDS,
		}
	}
}

/// A limit independently enforced by the installer profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Budget {
	Diagnostics,
	PlistBytes,
	TotalPlistBytes,
	PlistEvents,
	PlistDepth,
	PlistExpandedBytes,
	Backgrounds,
}

/// A failure that prevented the installer profile from finishing.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Manifest(#[from] manifest::Error),
	#[error("could not inspect installer source at {destination:?}")]
	SourceIo {
		destination: String,
		#[source]
		source: io::Error,
	},
	#[error("installer lint reached its {budget:?} limit of {limit}")]
	Limit { budget: Budget, limit: u64 },
	#[error("could not reserve a bounded installer lint buffer")]
	Allocation(#[from] TryReserveError),
	#[error("installer lint was cancelled")]
	Cancelled,
	#[error("suppression for {0:?} requires a reason")]
	SuppressionReason(RuleId),
	#[error("installer lint has multiple suppressions for {0:?}")]
	DuplicateSuppression(RuleId),
}

/// Discovers the installer tree and checks its links, bundle metadata, and
/// explicitly declared image background sources without changing project files.
///
/// `config_dir` is the absolute directory containing the project manifest.
/// Findings describe the resolved copy filters and explicit project entries.
///
/// # Errors
/// Returns source discovery, identity, I/O, budget, and cancellation errors
/// without producing a partial report.
pub fn lint_installer<'input>(
	project: &Project,
	config_dir: &Path,
	limits: Limits,
	suppressions: &[Suppression<'input>],
	mut cancelled: impl FnMut() -> bool,
) -> Result<Report<'input>, Error> {
	check_cancellation(&mut cancelled)?;
	for (index, suppression) in suppressions.iter().enumerate() {
		if suppressions[..index]
			.iter()
			.any(|earlier| earlier.rule == suppression.rule)
		{
			return Err(Error::DuplicateSuppression(suppression.rule));
		}
	}
	let manifest =
		manifest::discover_for_inspection(project, config_dir, limits.manifest, &mut cancelled)
			.map_err(|source| match source {
				manifest::Error::Cancelled => Error::Cancelled,
				_ => Error::Manifest(source),
			})?;
	let mut findings = Findings {
		diagnostics: Vec::new(),
		suppressions,
		limit: limits.max_diagnostics,
	};
	paths::check_paths(
		manifest.entries(),
		|evidence| findings.add(evidence),
		&mut cancelled,
	)?;
	bundles::check(
		&manifest,
		project.metadata.preserve_permissions,
		limits,
		|evidence| findings.add(evidence),
		&mut cancelled,
	)?;
	let backgrounds = backgrounds::check(
		project,
		config_dir,
		limits,
		|evidence| findings.add(evidence),
		&mut cancelled,
	)?;
	for source in manifest
		.entries()
		.iter()
		.filter_map(|entry| entry.source.as_ref())
		.chain(&backgrounds)
	{
		check_cancellation(&mut cancelled)?;
		source.validate()?;
	}
	check_cancellation(&mut cancelled)?;
	Ok(Report {
		schema_version: SCHEMA_VERSION,
		diagnostics: findings.diagnostics,
	})
}

struct Findings<'policy, 'input> {
	diagnostics: Vec<Diagnostic<'input>>,
	suppressions: &'policy [Suppression<'input>],
	limit: usize,
}

impl Findings<'_, '_> {
	fn add(&mut self, evidence: Evidence) -> Result<(), Error> {
		if self.diagnostics.len() == self.limit {
			return Err(Error::Limit {
				budget: Budget::Diagnostics,
				limit: self.limit as u64,
			});
		}
		let rule = evidence.rule();
		let suppression_reason = self
			.suppressions
			.iter()
			.find(|item| item.rule == rule)
			.map(|item| item.reason);
		self.diagnostics.try_reserve(1)?;
		self.diagnostics.push(Diagnostic {
			rule,
			severity: rule.severity(),
			evidence,
			suppression_reason,
		});
		Ok(())
	}
}

fn check_cancellation(cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
	if cancelled() {
		Err(Error::Cancelled)
	} else {
		Ok(())
	}
}
