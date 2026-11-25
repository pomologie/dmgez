//! Bounded diagnostics for dmgez projects (D-REL-04).

#[cfg(feature = "installer")]
pub mod installer;
mod layout;
mod report;

pub use layout::{Budget, Error, Limits, Suppression, lint_layout};
pub use report::{
	Bounds, Diagnostic, Evidence, Icon, ParseRuleIdError, Report, RuleId, SCHEMA_VERSION, Severity,
};
