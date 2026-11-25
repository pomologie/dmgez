//! Bounded diagnostics for the configured Finder icon layout (D-REL-04).

mod layout;
mod report;

pub use layout::{Budget, Error, Limits, Suppression, lint_layout};
pub use report::{Bounds, Diagnostic, Evidence, Icon, Report, RuleId, SCHEMA_VERSION, Severity};
