use std::str::FromStr;

use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::Severity;

pub(super) const SCHEMA_VERSION: u32 = 1;

/// Stable identifiers used by the installer profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RuleId {
	MissingApplicationsLink,
	InvalidApplicationsLink,
	ExternalSymlink,
	BundleInfoMissing,
	BundleInfoInvalid,
	BundleExecutableMissing,
	BundleExecutableWrongKind,
	BundleExecutableNotExecutable,
	BundleExecutableModeUnavailable,
	BundleExecutablePermissionsNotPreserved,
	BackgroundMissing,
	BackgroundInvalidSource,
}

impl RuleId {
	const ALL: &[Self] = &[
		Self::MissingApplicationsLink,
		Self::InvalidApplicationsLink,
		Self::ExternalSymlink,
		Self::BundleInfoMissing,
		Self::BundleInfoInvalid,
		Self::BundleExecutableMissing,
		Self::BundleExecutableWrongKind,
		Self::BundleExecutableNotExecutable,
		Self::BundleExecutableModeUnavailable,
		Self::BundleExecutablePermissionsNotPreserved,
		Self::BackgroundMissing,
		Self::BackgroundInvalidSource,
	];
	/// Returns the case-sensitive rule identifier accepted by the CLI.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::MissingApplicationsLink => "missing_applications_link",
			Self::InvalidApplicationsLink => "invalid_applications_link",
			Self::ExternalSymlink => "external_symlink",
			Self::BundleInfoMissing => "bundle_info_missing",
			Self::BundleInfoInvalid => "bundle_info_invalid",
			Self::BundleExecutableMissing => "bundle_executable_missing",
			Self::BundleExecutableWrongKind => "bundle_executable_wrong_kind",
			Self::BundleExecutableNotExecutable => "bundle_executable_not_executable",
			Self::BundleExecutableModeUnavailable => "bundle_executable_mode_unavailable",
			Self::BundleExecutablePermissionsNotPreserved => {
				"bundle_executable_permissions_not_preserved"
			}
			Self::BackgroundMissing => "background_missing",
			Self::BackgroundInvalidSource => "background_invalid_source",
		}
	}

	#[must_use]
	pub const fn severity(self) -> Severity {
		Severity::Warning
	}
}

/// The requested rule is outside the installer profile.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("unknown installer lint rule identifier")]
pub struct ParseRuleIdError;

impl FromStr for RuleId {
	type Err = ParseRuleIdError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		Self::ALL
			.iter()
			.copied()
			.find(|rule| rule.as_str() == value)
			.ok_or(ParseRuleIdError)
	}
}

impl Serialize for RuleId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(self.as_str())
	}
}

/// Reason a bundle's Info.plist could not establish a safe executable name.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum BundleInfoProblem {
	MissingSource,
	NotRegularFile,
	InvalidPlist,
	MissingExecutable,
	InvalidExecutable,
}

/// Which explicitly configured image variant was inspected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum BackgroundScale {
	Base,
	Retina,
}

/// Installer findings with exact virtual destinations and literal link bytes.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
	MissingApplicationsLink {
		destination: String,
	},
	InvalidApplicationsLink {
		destination: String,
		/// `None` when the destination is not a symbolic link.
		target: Option<Vec<u8>>,
	},
	ExternalSymlink {
		destination: String,
		target: Vec<u8>,
	},
	BundleInfoMissing {
		bundle: String,
		destination: String,
	},
	BundleInfoInvalid {
		bundle: String,
		destination: String,
		reason: BundleInfoProblem,
	},
	BundleExecutableMissing {
		bundle: String,
		destination: String,
	},
	BundleExecutableWrongKind {
		bundle: String,
		destination: String,
	},
	BundleExecutableNotExecutable {
		bundle: String,
		destination: String,
		mode: u16,
	},
	BundleExecutableModeUnavailable {
		bundle: String,
		destination: String,
	},
	BundleExecutablePermissionsNotPreserved {
		bundle: String,
		destination: String,
	},
	BackgroundMissing {
		destination: String,
		source: String,
		scale: BackgroundScale,
	},
	BackgroundInvalidSource {
		destination: String,
		source: String,
		scale: BackgroundScale,
	},
}

impl Evidence {
	/// Returns the virtual path associated with the finding.
	#[must_use]
	pub fn destination(&self) -> &str {
		match self {
			Self::MissingApplicationsLink { destination }
			| Self::InvalidApplicationsLink { destination, .. }
			| Self::ExternalSymlink { destination, .. }
			| Self::BundleInfoMissing { destination, .. }
			| Self::BundleInfoInvalid { destination, .. }
			| Self::BundleExecutableMissing { destination, .. }
			| Self::BundleExecutableWrongKind { destination, .. }
			| Self::BundleExecutableNotExecutable { destination, .. }
			| Self::BundleExecutableModeUnavailable { destination, .. }
			| Self::BundleExecutablePermissionsNotPreserved { destination, .. }
			| Self::BackgroundMissing { destination, .. }
			| Self::BackgroundInvalidSource { destination, .. } => destination,
		}
	}

	#[must_use]
	pub const fn rule(&self) -> RuleId {
		match self {
			Self::MissingApplicationsLink { .. } => RuleId::MissingApplicationsLink,
			Self::InvalidApplicationsLink { .. } => RuleId::InvalidApplicationsLink,
			Self::ExternalSymlink { .. } => RuleId::ExternalSymlink,
			Self::BundleInfoMissing { .. } => RuleId::BundleInfoMissing,
			Self::BundleInfoInvalid { .. } => RuleId::BundleInfoInvalid,
			Self::BundleExecutableMissing { .. } => RuleId::BundleExecutableMissing,
			Self::BundleExecutableWrongKind { .. } => RuleId::BundleExecutableWrongKind,
			Self::BundleExecutableNotExecutable { .. } => RuleId::BundleExecutableNotExecutable,
			Self::BundleExecutableModeUnavailable { .. } => RuleId::BundleExecutableModeUnavailable,
			Self::BundleExecutablePermissionsNotPreserved { .. } => {
				RuleId::BundleExecutablePermissionsNotPreserved
			}
			Self::BackgroundMissing { .. } => RuleId::BackgroundMissing,
			Self::BackgroundInvalidSource { .. } => RuleId::BackgroundInvalidSource,
		}
	}
}

/// A finding, including any reason supplied to suppress it.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic<'input> {
	pub rule: RuleId,
	pub severity: Severity,
	pub evidence: Evidence,
	pub suppression_reason: Option<&'input str>,
}

/// Completed installer checks over the effective project tree.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Report<'input> {
	pub schema_version: u32,
	pub diagnostics: Vec<Diagnostic<'input>>,
}

impl Report<'_> {
	/// Whether at least one warning is unsuppressed.
	#[must_use]
	pub fn has_warnings(&self) -> bool {
		self.diagnostics.iter().any(|finding| {
			finding.severity == Severity::Warning && finding.suppression_reason.is_none()
		})
	}
}
