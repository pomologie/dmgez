//! Bounded diagnostics for the configured Finder icon layout (D-REL-04).

use std::collections::TryReserveError;

use dmgez_preview::{FolderRect, GeometryError};
use serde::Serialize;
use thiserror::Error;

mod layout;

pub use layout::lint_layout;

/// Version of the serialized layout report.
pub const SCHEMA_VERSION: u32 = 1;

const DEFAULT_MAX_ITEMS: usize = 100_000;
const DEFAULT_MAX_PAIR_CHECKS: usize = 1_000_000;
const DEFAULT_MAX_DIAGNOSTICS: usize = 10_000;

/// Stable identifiers for the implemented layout rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RuleId {
	IconClipped,
	IconOutsideViewport,
	IconsOverlap,
	HiddenPositionedIcon,
	AutomaticArrangement,
	TightGridSpacing,
}

impl RuleId {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::IconClipped => "icon_clipped",
			Self::IconOutsideViewport => "icon_outside_viewport",
			Self::IconsOverlap => "icons_overlap",
			Self::HiddenPositionedIcon => "hidden_positioned_icon",
			Self::AutomaticArrangement => "automatic_arrangement",
			Self::TightGridSpacing => "tight_grid_spacing",
		}
	}

	#[must_use]
	pub const fn severity(self) -> Severity {
		match self {
			Self::HiddenPositionedIcon => Severity::Info,
			_ => Severity::Warning,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Severity {
	Info,
	Warning,
}

/// A rule-wide suppression supplied by the caller, with an exact reason.
#[derive(Clone, Copy, Debug)]
pub struct Suppression<'input> {
	rule: RuleId,
	reason: &'input str,
}

impl<'input> Suppression<'input> {
	/// Creates a suppression with a caller-supplied reason.
	/// # Errors
	/// Returns an error for an empty or whitespace-only reason.
	pub fn new(rule: RuleId, reason: &'input str) -> Result<Self, Error> {
		if reason.trim().is_empty() {
			return Err(Error::SuppressionReason(rule));
		}
		Ok(Self { rule, reason })
	}
}

/// Bounds for one lint operation over an already resolved folder.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	pub max_items: usize,
	/// Maximum candidate pairs checked after pruning disjoint horizontal spans.
	pub max_pair_checks: usize,
	/// Maximum findings retained, including suppressed findings.
	pub max_diagnostics: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_items: DEFAULT_MAX_ITEMS,
			max_pair_checks: DEFAULT_MAX_PAIR_CHECKS,
			max_diagnostics: DEFAULT_MAX_DIAGNOSTICS,
		}
	}
}

/// The exact inspected geometry in logical folder points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bounds {
	pub min: [f64; 2],
	pub max: [f64; 2],
}

impl From<FolderRect> for Bounds {
	fn from(rect: FolderRect) -> Self {
		Self {
			min: [rect.min().x(), rect.min().y()],
			max: [rect.max().x(), rect.max().y()],
		}
	}
}

/// A configured icon's virtual destination and placed rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Icon<'input> {
	pub destination: &'input str,
	pub bounds: Bounds,
}

/// Rule-specific evidence derived from the selected preview geometry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence<'input> {
	IconClipped {
		icon: Icon<'input>,
	},
	IconOutsideViewport {
		icon: Icon<'input>,
	},
	IconsOverlap {
		first: Icon<'input>,
		second: Icon<'input>,
	},
	HiddenPositionedIcon {
		destination: &'input str,
	},
	AutomaticArrangement {
		configured_visible_icons: usize,
	},
	TightGridSpacing {
		spacing_points: f64,
		recommended_minimum_points: f64,
	},
}

impl Evidence<'_> {
	#[must_use]
	pub const fn rule(self) -> RuleId {
		match self {
			Self::IconClipped { .. } => RuleId::IconClipped,
			Self::IconOutsideViewport { .. } => RuleId::IconOutsideViewport,
			Self::IconsOverlap { .. } => RuleId::IconsOverlap,
			Self::HiddenPositionedIcon { .. } => RuleId::HiddenPositionedIcon,
			Self::AutomaticArrangement { .. } => RuleId::AutomaticArrangement,
			Self::TightGridSpacing { .. } => RuleId::TightGridSpacing,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic<'input> {
	pub rule: RuleId,
	pub severity: Severity,
	pub evidence: Evidence<'input>,
	/// Present when the caller suppressed this rule. The finding remains visible.
	pub suppression_reason: Option<&'input str>,
}

/// Findings for the configured icon layout of one folder.
#[derive(Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Report<'input> {
	pub schema_version: u32,
	/// Initial content viewport using the configured scroll origin.
	pub viewport: Bounds,
	pub diagnostics: Vec<Diagnostic<'input>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Budget {
	Items,
	PairChecks,
	Diagnostics,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Geometry(#[from] GeometryError),
	#[error("layout lint reached its {budget:?} limit of {limit}")]
	Limit { budget: Budget, limit: usize },
	#[error("could not reserve a bounded layout lint buffer")]
	Allocation(#[from] TryReserveError),
	#[error("layout lint was cancelled")]
	Cancelled,
	#[error("suppression for {0:?} requires a reason")]
	SuppressionReason(RuleId),
	#[error("layout lint has multiple suppressions for {0:?}")]
	DuplicateSuppression(RuleId),
}
